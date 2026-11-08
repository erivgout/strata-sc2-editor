//! BLTE: the block-table encoding every CASC payload is wrapped in.
//!
//! Layout (all integers big-endian):
//! - `"BLTE"`, `u32 header_size`.
//! - `header_size == 0`: the rest is a single frame.
//! - otherwise: `u8 0x0F`, `u24 frame_count`, then per frame
//!   `u32 encoded_size, u32 content_size, [u8; 16] md5`, then the frames.
//!
//! Each frame starts with a mode byte: `N` raw, `Z` zlib, `F` a nested BLTE
//! stream, `E` encrypted (unsupported: we never need encrypted SC2 data, so
//! the frame is reported as [`CascError::Encrypted`]).

use crate::CascError;

/// Decodes a complete BLTE stream into the file's content bytes.
pub fn decode(data: &[u8]) -> Result<Vec<u8>, CascError> {
    let mut out = Vec::new();
    decode_into(data, &mut out)?;
    Ok(out)
}

fn be32(b: &[u8]) -> u32 {
    u32::from_be_bytes([b[0], b[1], b[2], b[3]])
}

fn decode_into(data: &[u8], out: &mut Vec<u8>) -> Result<(), CascError> {
    if data.len() < 8 || &data[..4] != b"BLTE" {
        return Err(CascError::Format("missing BLTE signature".into()));
    }
    let header_size = be32(&data[4..8]) as usize;
    if header_size == 0 {
        return decode_frame(&data[8..], None, out);
    }
    if header_size < 12 || header_size > data.len() || data[8] != 0x0F {
        return Err(CascError::Format("bad BLTE header".into()));
    }
    let frame_count = u32::from_be_bytes([0, data[9], data[10], data[11]]) as usize;
    if 12 + frame_count * 24 > header_size {
        return Err(CascError::Format("BLTE frame table overruns header".into()));
    }
    let mut pos = header_size;
    for i in 0..frame_count {
        let rec = &data[12 + i * 24..12 + (i + 1) * 24];
        let encoded = be32(&rec[0..4]) as usize;
        let content = be32(&rec[4..8]) as usize;
        let frame = data
            .get(pos..pos + encoded)
            .ok_or_else(|| CascError::Format("BLTE frame runs past end of data".into()))?;
        decode_frame(frame, Some(content), out)?;
        pos += encoded;
    }
    Ok(())
}

fn decode_frame(frame: &[u8], content_size: Option<usize>, out: &mut Vec<u8>) -> Result<(), CascError> {
    let Some((&mode, body)) = frame.split_first() else {
        return Err(CascError::Format("empty BLTE frame".into()));
    };
    let start = out.len();
    match mode {
        b'N' => out.extend_from_slice(body),
        b'Z' => {
            let bytes = miniz_oxide::inflate::decompress_to_vec_zlib(body)
                .map_err(|e| CascError::Format(format!("zlib frame: {e:?}")))?;
            out.extend_from_slice(&bytes);
        }
        b'F' => decode_into(body, out)?,
        b'E' => return Err(CascError::Encrypted),
        other => return Err(CascError::Format(format!("unsupported BLTE frame mode {:?}", other as char))),
    }
    if let Some(expected) = content_size {
        if out.len() - start != expected {
            return Err(CascError::Format(format!(
                "BLTE frame decoded to {} bytes, expected {expected}",
                out.len() - start
            )));
        }
    }
    Ok(())
}

/// Encodes `content` as BLTE with the given frame sizes, each frame either raw
/// (`N`) or zlib (`Z`). Only used to build test vectors; the editor never
/// writes CASC.
#[cfg(test)]
pub(crate) fn encode(frames: &[(&[u8], bool)]) -> Vec<u8> {
    let mut bodies = Vec::new();
    for (bytes, zlib) in frames {
        let mut f = Vec::new();
        if *zlib {
            f.push(b'Z');
            f.extend_from_slice(&miniz_oxide::deflate::compress_to_vec_zlib(bytes, 6));
        } else {
            f.push(b'N');
            f.extend_from_slice(bytes);
        }
        bodies.push((f, bytes.len()));
    }
    let header_size = 12 + 24 * bodies.len();
    let mut out = b"BLTE".to_vec();
    out.extend_from_slice(&(header_size as u32).to_be_bytes());
    out.push(0x0F);
    out.extend_from_slice(&(bodies.len() as u32).to_be_bytes()[1..]);
    for (f, content) in &bodies {
        out.extend_from_slice(&(f.len() as u32).to_be_bytes());
        out.extend_from_slice(&(*content as u32).to_be_bytes());
        out.extend_from_slice(&[0u8; 16]);
    }
    for (f, _) in &bodies {
        out.extend_from_slice(f);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_raw_frame_without_table() {
        let mut data = b"BLTE\0\0\0\0N".to_vec();
        data.extend_from_slice(b"hello");
        assert_eq!(decode(&data).unwrap(), b"hello");
    }

    #[test]
    fn single_zlib_frame_without_table() {
        let mut data = b"BLTE\0\0\0\0Z".to_vec();
        data.extend_from_slice(&miniz_oxide::deflate::compress_to_vec_zlib(b"<Catalog/>", 6));
        assert_eq!(decode(&data).unwrap(), b"<Catalog/>");
    }

    #[test]
    fn multi_frame_mixed_modes() {
        let data = encode(&[(b"<?xml ", false), (b"version=\"1.0\"?>", true), (b"<Catalog/>", true)]);
        assert_eq!(decode(&data).unwrap(), b"<?xml version=\"1.0\"?><Catalog/>");
    }

    #[test]
    fn nested_frame() {
        let inner = encode(&[(b"inner", true)]);
        let mut frame = vec![b'F'];
        frame.extend_from_slice(&inner);
        let mut data = b"BLTE\0\0\0\0".to_vec();
        data.extend_from_slice(&frame);
        assert_eq!(decode(&data).unwrap(), b"inner");
    }

    #[test]
    fn encrypted_and_garbage_are_errors() {
        assert!(matches!(decode(b"BLTE\0\0\0\0E1234"), Err(CascError::Encrypted)));
        assert!(decode(b"NOPE").is_err());
        let mut bad = encode(&[(b"abc", false)]);
        bad.truncate(bad.len() - 1);
        assert!(decode(&bad).is_err());
    }
}
