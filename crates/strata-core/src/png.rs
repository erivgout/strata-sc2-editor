//! A small PNG encoder (8-bit RGB or RGBA, zlib via `miniz_oxide`), for map
//! images written by the app and returned by the MCP server.

/// Encode `pixels` (rows top to bottom, `channels` bytes per pixel: 3 = RGB,
/// 4 = RGBA) as a PNG file.
pub fn encode(width: u32, height: u32, channels: u8, pixels: &[u8]) -> Result<Vec<u8>, String> {
    let color_type = match channels {
        3 => 2u8,
        4 => 6u8,
        _ => return Err(format!("PNG needs 3 or 4 channels, not {channels}")),
    };
    let stride = width as usize * channels as usize;
    if width == 0 || height == 0 || pixels.len() != stride * height as usize {
        return Err(format!("{} bytes is not a {width}x{height} image with {channels} channels", pixels.len()));
    }
    // Each row starts with its filter byte. "Up" filtering (1) compresses
    // smooth terrain well and is cheap.
    let mut raw = Vec::with_capacity((stride + 1) * height as usize);
    for y in 0..height as usize {
        let row = &pixels[y * stride..(y + 1) * stride];
        if y == 0 {
            raw.push(0);
            raw.extend_from_slice(row);
        } else {
            let prev = &pixels[(y - 1) * stride..y * stride];
            raw.push(2);
            raw.extend(row.iter().zip(prev).map(|(a, b)| a.wrapping_sub(*b)));
        }
    }
    let data = miniz_oxide::deflate::compress_to_vec_zlib(&raw, 6);

    let mut out = Vec::with_capacity(data.len() + 64);
    out.extend_from_slice(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]);
    let mut ihdr = Vec::with_capacity(13);
    ihdr.extend_from_slice(&width.to_be_bytes());
    ihdr.extend_from_slice(&height.to_be_bytes());
    ihdr.extend_from_slice(&[8, color_type, 0, 0, 0]);
    chunk(&mut out, b"IHDR", &ihdr);
    chunk(&mut out, b"IDAT", &data);
    chunk(&mut out, b"IEND", &[]);
    Ok(out)
}

fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    out.extend_from_slice(kind);
    out.extend_from_slice(data);
    let mut crc = crc32(kind, 0xFFFF_FFFF);
    crc = crc32(data, crc);
    out.extend_from_slice(&(!crc).to_be_bytes());
}

fn crc32(bytes: &[u8], mut crc: u32) -> u32 {
    for &b in bytes {
        crc ^= b as u32;
        for _ in 0..8 {
            crc = if crc & 1 != 0 { 0xEDB8_8320 ^ (crc >> 1) } else { crc >> 1 };
        }
    }
    crc
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_a_decodable_png() {
        let (w, h) = (3u32, 2u32);
        let px: Vec<u8> = (0..w * h * 3).map(|i| (i * 17) as u8).collect();
        let png = encode(w, h, 3, &px).unwrap();
        assert_eq!(&png[..8], &[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]);
        // CRC of the IEND chunk type, a well-known constant.
        assert_eq!(crc32(b"IEND", 0xFFFF_FFFF) ^ 0xFFFF_FFFF, 0xAE42_6082);
        // Undo the filters and compare.
        let idat_len = u32::from_be_bytes(png[33..37].try_into().unwrap()) as usize;
        assert_eq!(&png[37..41], b"IDAT");
        let raw = miniz_oxide::inflate::decompress_to_vec_zlib(&png[41..41 + idat_len]).unwrap();
        let stride = (w * 3) as usize;
        let mut prev = vec![0u8; stride];
        let mut back = Vec::new();
        for row in raw.chunks(stride + 1) {
            let line: Vec<u8> = match row[0] {
                0 => row[1..].to_vec(),
                2 => row[1..].iter().zip(&prev).map(|(a, b)| a.wrapping_add(*b)).collect(),
                f => panic!("filter {f}"),
            };
            back.extend_from_slice(&line);
            prev = line;
        }
        assert_eq!(back, px);
        assert!(encode(2, 2, 3, &px).is_err());
    }
}
