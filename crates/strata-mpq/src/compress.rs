//! Sector (de)compression: the multi-compression mask byte used by
//! `MPQ_FILE_COMPRESS`, plus PKWARE implode and LZMA.

use crate::MpqError;

pub const COMP_HUFFMAN: u8 = 0x01;
pub const COMP_ZLIB: u8 = 0x02;
pub const COMP_PKWARE: u8 = 0x08;
pub const COMP_BZIP2: u8 = 0x10;
pub const COMP_SPARSE: u8 = 0x20;
pub const COMP_ADPCM_MONO: u8 = 0x40;
pub const COMP_ADPCM_STEREO: u8 = 0x80;
/// LZMA is a standalone method, not a bit combination.
pub const COMP_LZMA: u8 = 0x12;

fn corrupt(what: &str) -> MpqError {
    MpqError::Corrupt(what.to_owned())
}

/// Decompress one sector whose first byte is the compression mask.
/// `out.len()` is the expected decompressed size; it must be filled exactly.
pub fn decompress_sector(input: &[u8], out: &mut [u8]) -> Result<(), MpqError> {
    let (&mask, data) = input
        .split_first()
        .ok_or_else(|| corrupt("empty compressed sector"))?;
    if mask == COMP_LZMA {
        return decompress_lzma(data, out);
    }
    if mask & (COMP_HUFFMAN | COMP_ADPCM_MONO | COMP_ADPCM_STEREO) != 0 {
        return Err(MpqError::Unsupported(format!(
            "Huffman/ADPCM sector compression (mask {mask:#04x})"
        )));
    }
    if mask & !(COMP_ZLIB | COMP_PKWARE | COMP_BZIP2 | COMP_SPARSE) != 0 {
        return Err(MpqError::Unsupported(format!(
            "unknown compression mask {mask:#04x}"
        )));
    }

    // Storm decompression order: bzip2, pkware, zlib, (huffman, adpcm), sparse.
    let steps: Vec<u8> = [COMP_BZIP2, COMP_PKWARE, COMP_ZLIB, COMP_SPARSE]
        .into_iter()
        .filter(|s| mask & s != 0)
        .collect();
    match steps.len() {
        0 => {
            // Mask 0: stored inside a "compressed" sector.
            if data.len() != out.len() {
                return Err(corrupt("uncompressed sector has wrong size"));
            }
            out.copy_from_slice(data);
            Ok(())
        }
        1 => {
            let n = run_step(steps[0], data, out)?;
            if n != out.len() {
                return Err(corrupt("sector decompressed to wrong size"));
            }
            Ok(())
        }
        _ => {
            // Intermediate stages may be slightly larger than the final data.
            let cap = out.len() + out.len() / 64 + 64;
            let mut cur: Vec<u8> = data.to_vec();
            let last = steps.len() - 1;
            for (i, &step) in steps.iter().enumerate() {
                if i == last {
                    let n = run_step(step, &cur, out)?;
                    if n != out.len() {
                        return Err(corrupt("sector decompressed to wrong size"));
                    }
                } else {
                    let mut next = vec![0u8; cap];
                    let n = run_step(step, &cur, &mut next)?;
                    next.truncate(n);
                    cur = next;
                }
            }
            Ok(())
        }
    }
}

/// Run one decompression step into `out`, returning the bytes produced.
fn run_step(step: u8, input: &[u8], out: &mut [u8]) -> Result<usize, MpqError> {
    match step {
        COMP_ZLIB => inflate_zlib(input, out),
        COMP_BZIP2 => decompress_bzip2(input, out),
        COMP_PKWARE => {
            crate::explode::explode(input, out).ok_or_else(|| corrupt("bad PKWARE data"))
        }
        COMP_SPARSE => decompress_sparse(input, out),
        _ => unreachable!("filtered above"),
    }
}

fn inflate_zlib(input: &[u8], out: &mut [u8]) -> Result<usize, MpqError> {
    miniz_oxide::inflate::decompress_slice_iter_to_slice(out, std::iter::once(input), true, true)
        .map_err(|e| MpqError::Corrupt(format!("zlib: {e:?}")))
}

fn decompress_bzip2(input: &[u8], out: &mut [u8]) -> Result<usize, MpqError> {
    let too_big = || MpqError::Unsupported("bzip2 sector over 4 GiB".into());
    let mut dest_len = u32::try_from(out.len()).map_err(|_| too_big())?;
    let src_len = u32::try_from(input.len()).map_err(|_| too_big())?;
    // SAFETY: `out` is writable for `dest_len` bytes and `input` readable for
    // `src_len` bytes; libbzip2 never writes through the source pointer.
    let ret = unsafe {
        libbz2_rs_sys::BZ2_bzBuffToBuffDecompress(
            out.as_mut_ptr().cast(),
            &mut dest_len,
            input.as_ptr().cast_mut().cast(),
            src_len,
            0,
            0,
        )
    };
    if ret != libbz2_rs_sys::BZ_OK {
        return Err(MpqError::Corrupt(format!("bzip2 error {ret}")));
    }
    Ok(dest_len as usize)
}

/// MPQ LZMA: 1 filter byte (0 = none), 5 property bytes, 8-byte size, stream.
fn decompress_lzma(data: &[u8], out: &mut [u8]) -> Result<(), MpqError> {
    match data.first() {
        Some(0) => {}
        Some(f) => {
            return Err(MpqError::Unsupported(format!("LZMA filter {f}")));
        }
        None => return Err(corrupt("empty LZMA sector")),
    }
    let options = lzma_rs::decompress::Options {
        unpacked_size: lzma_rs::decompress::UnpackedSize::ReadHeaderButUseProvided(Some(
            out.len() as u64
        )),
        ..Default::default()
    };
    let mut input = &data[1..];
    let mut writer: &mut [u8] = out;
    lzma_rs::lzma_decompress_with_options(&mut input, &mut writer, &options)
        .map_err(|e| MpqError::Corrupt(format!("lzma: {e}")))?;
    if !writer.is_empty() {
        return Err(corrupt("LZMA sector decompressed to wrong size"));
    }
    Ok(())
}

/// Blizzard "sparse" RLE: 4-byte big-endian size, then chunks where a byte
/// with the high bit set copies `(b & 0x7F) + 1` literals, otherwise writes
/// `(b & 0x7F) + 3` zeros.
fn decompress_sparse(input: &[u8], out: &mut [u8]) -> Result<usize, MpqError> {
    if input.len() < 4 {
        return Err(corrupt("sparse header"));
    }
    let size = u32::from_be_bytes([input[0], input[1], input[2], input[3]]) as usize;
    let size = size.min(out.len());
    let mut i = 4;
    let mut n = 0;
    while i < input.len() && n < size {
        let b = input[i];
        i += 1;
        if b & 0x80 != 0 {
            let len = ((b & 0x7F) as usize + 1).min(size - n);
            let src = input
                .get(i..i + len)
                .ok_or_else(|| corrupt("sparse literal overrun"))?;
            out[n..n + len].copy_from_slice(src);
            i += (b & 0x7F) as usize + 1;
            n += len;
        } else {
            let len = ((b & 0x7F) as usize + 3).min(size - n);
            out[n..n + len].fill(0);
            n += len;
        }
    }
    Ok(n)
}

/// Compress one sector with zlib, returning `[COMP_ZLIB, zlib stream...]`
/// only if that is strictly smaller than the input.
pub fn compress_sector_zlib(input: &[u8], level: u8) -> Option<Vec<u8>> {
    let z = miniz_oxide::deflate::compress_to_vec_zlib(input, level);
    if z.len() + 1 < input.len() {
        let mut v = Vec::with_capacity(z.len() + 1);
        v.push(COMP_ZLIB);
        v.extend_from_slice(&z);
        Some(v)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zlib_roundtrip() {
        let data: Vec<u8> = (0..5000u32).map(|i| (i % 7) as u8).collect();
        let c = compress_sector_zlib(&data, 6).unwrap();
        let mut out = vec![0u8; data.len()];
        decompress_sector(&c, &mut out).unwrap();
        assert_eq!(out, data);
    }

    #[test]
    fn sparse_decode() {
        // size 8: 2 literals "ab", then 6 zeros.
        let input = [COMP_SPARSE, 0, 0, 0, 8, 0x81, b'a', b'b', 0x03];
        let mut out = [0xFFu8; 8];
        decompress_sector(&input, &mut out).unwrap();
        assert_eq!(&out, b"ab\0\0\0\0\0\0");
    }

    #[test]
    fn sparse_then_zlib() {
        let sparse = [0u8, 0, 0, 10, 0x80, b'x', 0x06];
        let z = miniz_oxide::deflate::compress_to_vec_zlib(&sparse, 6);
        let mut input = vec![COMP_SPARSE | COMP_ZLIB];
        input.extend_from_slice(&z);
        let mut out = [0xFFu8; 10];
        decompress_sector(&input, &mut out).unwrap();
        assert_eq!(&out, b"x\0\0\0\0\0\0\0\0\0");
    }

    #[test]
    fn lzma_decode() {
        let data: Vec<u8> = (0..3000u32).map(|i| (i * 31 % 251) as u8).collect();
        let mut stream = Vec::new();
        lzma_rs::lzma_compress(&mut &data[..], &mut stream).unwrap();
        let mut input = vec![COMP_LZMA, 0];
        input.extend_from_slice(&stream);
        let mut out = vec![0u8; data.len()];
        decompress_sector(&input, &mut out).unwrap();
        assert_eq!(out, data);
    }

    #[test]
    fn bzip2_decode() {
        // Python: bz2.compress(b'hello hello hello hello\n' * 3, 9)
        const BZ: &[u8] = &[
            0x42, 0x5a, 0x68, 0x39, 0x31, 0x41, 0x59, 0x26, 0x53, 0x59, 0x06, 0xd3, 0x95, 0xaa,
            0x00, 0x00, 0x10, 0xd1, 0x00, 0x00, 0x10, 0x40, 0x00, 0x02, 0x44, 0xa0, 0x00, 0x30,
            0xc0, 0x02, 0x95, 0x0c, 0x9a, 0x58, 0x5e, 0x8f, 0x58, 0x5a, 0x2f, 0x8b, 0xb9, 0x22,
            0x9c, 0x28, 0x48, 0x03, 0x69, 0xca, 0xd5, 0x00,
        ];
        let expected = b"hello hello hello hello\n".repeat(3);
        let mut input = vec![COMP_BZIP2];
        input.extend_from_slice(BZ);
        let mut out = vec![0u8; expected.len()];
        decompress_sector(&input, &mut out).unwrap();
        assert_eq!(&out[..], &expected[..]);
    }

    #[test]
    fn bzip2_multiblock_incompressible() {
        // Pseudo-random data spanning several 900k blocks (real maps store
        // .ogg files like this).
        let mut x = 0x1234_5678u32;
        let data: Vec<u8> = (0..2_000_000)
            .map(|_| {
                x ^= x << 13;
                x ^= x >> 17;
                x ^= x << 5;
                x as u8
            })
            .collect();
        let mut comp = vec![0u8; data.len() + data.len() / 50 + 1024];
        let mut comp_len = comp.len() as u32;
        let ret = unsafe {
            libbz2_rs_sys::BZ2_bzBuffToBuffCompress(
                comp.as_mut_ptr().cast(),
                &mut comp_len,
                data.as_ptr().cast_mut().cast(),
                data.len() as u32,
                9,
                0,
                0,
            )
        };
        assert_eq!(ret, libbz2_rs_sys::BZ_OK);
        let mut input = vec![COMP_BZIP2];
        input.extend_from_slice(&comp[..comp_len as usize]);
        let mut out = vec![0u8; data.len()];
        decompress_sector(&input, &mut out).unwrap();
        assert!(out == data);
    }
}
