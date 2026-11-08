//! DDS texture decoding to RGBA8 for StarCraft II assets (terrain textures,
//! model textures). Pure Rust; compiles for wasm32.
//!
//! Supports BC1 (DXT1), BC2 (DXT3), BC3 (DXT5), BC4 (ATI1/BC4U), BC5
//! (ATI2/BC5U), BC7 via the DX10 header, and uncompressed 32/24-bit RGB(A).
//! [`decode_level`] decodes a smaller mip directly, which saves memory and
//! upload time when full resolution isn't needed.

const MAGIC: &[u8; 4] = b"DDS ";
const DDPF_FOURCC: u32 = 0x4;
const DDPF_RGB: u32 = 0x40;
const DDPF_ALPHAPIXELS: u32 = 0x1;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum DdsError {
    #[error("not a DDS file")]
    NotDds,
    #[error("truncated DDS data")]
    Truncated,
    #[error("unsupported DDS format: {0}")]
    Unsupported(String),
}

/// A decoded image, rows top to bottom, 4 bytes per pixel (R, G, B, A).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Image {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Bc1,
    Bc2,
    Bc3,
    Bc4,
    Bc5,
    Bc7,
    /// Uncompressed with channel bit masks (r, g, b, a) and bytes per pixel.
    Rgb { bpp: u32, masks: [u32; 4] },
}

impl Format {
    /// Bytes for one mip level of `w` x `h` pixels.
    fn level_size(self, w: u32, h: u32) -> usize {
        let blocks = |w: u32, h: u32| (w.div_ceil(4).max(1) * h.div_ceil(4).max(1)) as usize;
        match self {
            Format::Bc1 | Format::Bc4 => blocks(w, h) * 8,
            Format::Bc2 | Format::Bc3 | Format::Bc5 | Format::Bc7 => blocks(w, h) * 16,
            Format::Rgb { bpp, .. } => (w.max(1) * h.max(1) * bpp) as usize,
        }
    }
}

/// Parsed DDS header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Header {
    pub width: u32,
    pub height: u32,
    pub mip_count: u32,
    pub format: Format,
    /// Offset of the first mip's data.
    pub data_offset: usize,
}

fn u32_at(b: &[u8], off: usize) -> Result<u32, DdsError> {
    b.get(off..off + 4).map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]])).ok_or(DdsError::Truncated)
}

pub fn parse_header(bytes: &[u8]) -> Result<Header, DdsError> {
    if bytes.get(0..4) != Some(MAGIC) {
        return Err(DdsError::NotDds);
    }
    let height = u32_at(bytes, 12)?;
    let width = u32_at(bytes, 16)?;
    let mip_count = u32_at(bytes, 28)?.max(1);
    let pf_flags = u32_at(bytes, 80)?;
    let fourcc = bytes.get(84..88).ok_or(DdsError::Truncated)?;
    let mut data_offset = 128;
    if width == 0 || height == 0 || width > 16384 || height > 16384 {
        return Err(DdsError::Unsupported(format!("size {width}x{height}")));
    }
    let format = if pf_flags & DDPF_FOURCC != 0 {
        match fourcc {
            b"DXT1" => Format::Bc1,
            b"DXT2" | b"DXT3" => Format::Bc2,
            b"DXT4" | b"DXT5" => Format::Bc3,
            b"ATI1" | b"BC4U" => Format::Bc4,
            b"ATI2" | b"BC5U" => Format::Bc5,
            b"DX10" => {
                data_offset = 148;
                match u32_at(bytes, 128)? {
                    70..=72 => Format::Bc1,
                    73..=75 => Format::Bc2,
                    76..=78 => Format::Bc3,
                    79..=81 => Format::Bc4,
                    82..=84 => Format::Bc5,
                    97..=99 => Format::Bc7,
                    28 | 29 => Format::Rgb { bpp: 4, masks: [0xFF, 0xFF00, 0xFF_0000, 0xFF00_0000] },
                    87 | 91 => Format::Rgb { bpp: 4, masks: [0xFF_0000, 0xFF00, 0xFF, 0xFF00_0000] },
                    other => return Err(DdsError::Unsupported(format!("DXGI format {other}"))),
                }
            }
            other => return Err(DdsError::Unsupported(format!("fourcc {:?}", String::from_utf8_lossy(other)))),
        }
    } else if pf_flags & DDPF_RGB != 0 {
        let bits = u32_at(bytes, 88)?;
        if bits != 32 && bits != 24 {
            return Err(DdsError::Unsupported(format!("{bits}-bit RGB")));
        }
        let a = if pf_flags & DDPF_ALPHAPIXELS != 0 { u32_at(bytes, 104)? } else { 0 };
        Format::Rgb { bpp: bits / 8, masks: [u32_at(bytes, 92)?, u32_at(bytes, 96)?, u32_at(bytes, 100)?, a] }
    } else {
        return Err(DdsError::Unsupported("pixel format without FourCC or RGB".into()));
    };
    Ok(Header { width, height, mip_count, format, data_offset })
}

/// Decode the full-resolution image.
pub fn decode(bytes: &[u8]) -> Result<Image, DdsError> {
    decode_level(bytes, 0)
}

/// Decode mip `level` (clamped to the last mip present). Level 1 is half size.
pub fn decode_level(bytes: &[u8], level: u32) -> Result<Image, DdsError> {
    let h = parse_header(bytes)?;
    let level = level.min(h.mip_count - 1);
    let (mut w, mut ht, mut off) = (h.width, h.height, h.data_offset);
    for _ in 0..level {
        off += h.format.level_size(w, ht);
        w = (w / 2).max(1);
        ht = (ht / 2).max(1);
    }
    let size = h.format.level_size(w, ht);
    let data = bytes.get(off..off + size).ok_or(DdsError::Truncated)?;
    decode_raw(data, w, ht, h.format)
}

/// Pick the mip whose larger side is at most `max_side` and decode it.
pub fn decode_max_side(bytes: &[u8], max_side: u32) -> Result<Image, DdsError> {
    let h = parse_header(bytes)?;
    let mut level = 0;
    let mut side = h.width.max(h.height);
    while side > max_side && level + 1 < h.mip_count {
        side /= 2;
        level += 1;
    }
    decode_level(bytes, level)
}

fn decode_raw(data: &[u8], w: u32, h: u32, format: Format) -> Result<Image, DdsError> {
    let (wu, hu) = (w as usize, h as usize);
    let rgba = match format {
        Format::Rgb { bpp, masks } => {
            let mut out = Vec::with_capacity(wu * hu * 4);
            let channel = |px: u32, mask: u32| -> u8 {
                if mask == 0 {
                    return 255;
                }
                // u64: a full 32-bit mask times 255 overflows u32.
                let shift = mask.trailing_zeros();
                let max = u64::from(mask >> shift);
                (u64::from((px & mask) >> shift) * 255 / max.max(1)) as u8
            };
            for p in data.chunks_exact(bpp as usize).take(wu * hu) {
                let px = match bpp {
                    4 => u32::from_le_bytes([p[0], p[1], p[2], p[3]]),
                    _ => u32::from_le_bytes([p[0], p[1], p[2], 0]),
                };
                out.extend_from_slice(&[
                    channel(px, masks[0]),
                    channel(px, masks[1]),
                    channel(px, masks[2]),
                    channel(px, masks[3]),
                ]);
            }
            out
        }
        bc => {
            let mut px = vec![0u32; wu * hu];
            let r = match bc {
                Format::Bc1 => texture2ddecoder::decode_bc1a(data, wu, hu, &mut px),
                Format::Bc2 => texture2ddecoder::decode_bc2(data, wu, hu, &mut px),
                Format::Bc3 => texture2ddecoder::decode_bc3(data, wu, hu, &mut px),
                Format::Bc4 => texture2ddecoder::decode_bc4(data, wu, hu, &mut px),
                Format::Bc5 => texture2ddecoder::decode_bc5(data, wu, hu, &mut px),
                Format::Bc7 => texture2ddecoder::decode_bc7(data, wu, hu, &mut px),
                Format::Rgb { .. } => unreachable!(),
            };
            r.map_err(|e| DdsError::Unsupported(e.to_owned()))?;
            // texture2ddecoder packs pixels as little-endian BGRA.
            let mut out = Vec::with_capacity(wu * hu * 4);
            for p in px {
                let [b, g, r, a] = p.to_le_bytes();
                out.extend_from_slice(&[r, g, b, a]);
            }
            out
        }
    };
    if rgba.len() != wu * hu * 4 {
        return Err(DdsError::Truncated);
    }
    Ok(Image { width: w, height: h, rgba })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn header(w: u32, h: u32, mips: u32, fourcc: &[u8; 4]) -> Vec<u8> {
        let mut b = vec![0u8; 128];
        b[0..4].copy_from_slice(MAGIC);
        b[4..8].copy_from_slice(&124u32.to_le_bytes());
        b[12..16].copy_from_slice(&h.to_le_bytes());
        b[16..20].copy_from_slice(&w.to_le_bytes());
        b[28..32].copy_from_slice(&mips.to_le_bytes());
        b[76..80].copy_from_slice(&32u32.to_le_bytes());
        b[80..84].copy_from_slice(&DDPF_FOURCC.to_le_bytes());
        b[84..88].copy_from_slice(fourcc);
        b
    }

    /// One BC1 block that is solid pure red (color0 = color1 = 0xF800).
    fn red_bc1_block() -> [u8; 8] {
        [0x00, 0xF8, 0x00, 0xF8, 0, 0, 0, 0]
    }

    #[test]
    fn decodes_bc1_and_mips() {
        // 8x8 with 2 mips: level 0 = 4 blocks, level 1 (4x4) = 1 block.
        let mut b = header(8, 8, 2, b"DXT1");
        for _ in 0..5 {
            b.extend_from_slice(&red_bc1_block());
        }
        let img = decode(&b).unwrap();
        assert_eq!((img.width, img.height), (8, 8));
        assert_eq!(&img.rgba[0..4], &[255, 0, 0, 255]);
        let small = decode_level(&b, 1).unwrap();
        assert_eq!((small.width, small.height), (4, 4));
        assert_eq!(decode_max_side(&b, 4).unwrap().width, 4);
        assert_eq!(decode_level(&b, 9).unwrap().width, 4, "level clamps to last mip");
    }

    #[test]
    fn decodes_uncompressed_bgra() {
        let mut b = header(1, 1, 1, b"\0\0\0\0");
        b[80..84].copy_from_slice(&(DDPF_RGB | DDPF_ALPHAPIXELS).to_le_bytes());
        b[88..92].copy_from_slice(&32u32.to_le_bytes());
        b[92..96].copy_from_slice(&0x00FF_0000u32.to_le_bytes());
        b[96..100].copy_from_slice(&0x0000_FF00u32.to_le_bytes());
        b[100..104].copy_from_slice(&0x0000_00FFu32.to_le_bytes());
        b[104..108].copy_from_slice(&0xFF00_0000u32.to_le_bytes());
        b.extend_from_slice(&[0x10, 0x20, 0x30, 0x40]); // B, G, R, A
        assert_eq!(decode(&b).unwrap().rgba, vec![0x30, 0x20, 0x10, 0x40]);
    }

    #[test]
    fn full_width_masks_do_not_overflow() {
        let mut b = header(1, 1, 1, b"\0\0\0\0");
        b[80..84].copy_from_slice(&(DDPF_RGB | DDPF_ALPHAPIXELS).to_le_bytes());
        b[88..92].copy_from_slice(&32u32.to_le_bytes());
        for off in [92, 96, 100, 104] {
            b[off..off + 4].copy_from_slice(&u32::MAX.to_le_bytes());
        }
        b.extend_from_slice(&u32::MAX.to_le_bytes());
        assert_eq!(decode(&b).unwrap().rgba, vec![255, 255, 255, 255]);
    }

    #[test]
    fn rejects_bad_input() {
        assert_eq!(decode(b"nope"), Err(DdsError::NotDds));
        let b = header(8, 8, 1, b"DXT1");
        assert_eq!(decode(&b), Err(DdsError::Truncated));
        let b = header(0, 8, 1, b"DXT1");
        assert!(matches!(parse_header(&b), Err(DdsError::Unsupported(_))));
    }
}
