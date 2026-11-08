//! PKWARE Data Compression Library "explode" decoder (port of zlib's
//! `contrib/blast/blast.c` by Mark Adler). Used by `MPQ_FILE_IMPLODE`
//! files and compression mask `0x08`.

const MAXBITS: usize = 13;

struct Huffman {
    count: [u16; MAXBITS + 1],
    symbol: Vec<u16>,
}

impl Huffman {
    /// Build a decoding table from blast's compact run-length representation.
    fn new(rep: &[u8]) -> Huffman {
        let mut lengths = Vec::with_capacity(256);
        for &r in rep {
            let len = r & 15;
            for _ in 0..(r >> 4) + 1 {
                lengths.push(len as usize);
            }
        }
        let mut count = [0u16; MAXBITS + 1];
        for &l in &lengths {
            count[l] += 1;
        }
        let mut offs = [0u16; MAXBITS + 1];
        for len in 1..MAXBITS {
            offs[len + 1] = offs[len] + count[len];
        }
        let mut symbol = vec![0u16; lengths.len()];
        for (sym, &l) in lengths.iter().enumerate() {
            if l != 0 {
                symbol[offs[l] as usize] = sym as u16;
                offs[l] += 1;
            }
        }
        Huffman { count, symbol }
    }
}

struct Bits<'a> {
    input: &'a [u8],
    pos: usize,
    buf: u32,
    cnt: u32,
}

impl Bits<'_> {
    fn bits(&mut self, need: u32) -> Option<u32> {
        while self.cnt < need {
            let byte = *self.input.get(self.pos)?;
            self.pos += 1;
            self.buf |= (byte as u32) << self.cnt;
            self.cnt += 8;
        }
        let val = self.buf & ((1u32 << need) - 1);
        self.buf >>= need;
        self.cnt -= need;
        Some(val)
    }

    /// Decode one symbol. PKWARE stores codes bit-inverted, MSB first.
    fn decode(&mut self, h: &Huffman) -> Option<u16> {
        let (mut code, mut first, mut index) = (0i32, 0i32, 0i32);
        for len in 1..=MAXBITS {
            code |= (self.bits(1)? ^ 1) as i32;
            let count = h.count[len] as i32;
            if code < first + count {
                return h.symbol.get((index + code - first) as usize).copied();
            }
            index += count;
            first += count;
            first <<= 1;
            code <<= 1;
        }
        None
    }
}

const LITLEN: [u8; 98] = [
    11, 124, 8, 7, 28, 7, 188, 13, 76, 4, 10, 8, 12, 10, 12, 10, 8, 23, 8, 9, 7, 6, 7, 8, 7, 6, 55,
    8, 23, 24, 12, 11, 7, 9, 11, 12, 6, 7, 22, 5, 7, 24, 6, 11, 9, 6, 7, 22, 7, 11, 38, 7, 9, 8,
    25, 11, 8, 11, 9, 12, 8, 12, 5, 38, 5, 38, 5, 11, 7, 5, 6, 21, 6, 10, 53, 8, 7, 24, 10, 27, 44,
    253, 253, 253, 252, 252, 252, 13, 12, 45, 12, 45, 12, 61, 12, 45, 44, 173,
];
const LENLEN: [u8; 6] = [2, 35, 36, 53, 38, 23];
const DISTLEN: [u8; 7] = [2, 20, 53, 230, 247, 151, 248];
const BASE: [u16; 16] = [3, 2, 4, 5, 6, 7, 8, 9, 10, 12, 16, 24, 40, 72, 136, 264];
const EXTRA: [u8; 16] = [0, 0, 0, 0, 0, 0, 0, 0, 1, 2, 3, 4, 5, 6, 7, 8];

/// Decompress a PKWARE DCL stream into `out`, stopping at the end code or
/// when `out` is full. Returns the number of bytes written.
pub fn explode(input: &[u8], out: &mut [u8]) -> Option<usize> {
    let litcode = Huffman::new(&LITLEN);
    let lencode = Huffman::new(&LENLEN);
    let distcode = Huffman::new(&DISTLEN);
    let mut s = Bits {
        input,
        pos: 0,
        buf: 0,
        cnt: 0,
    };
    let lit = s.bits(8)?;
    if lit > 1 {
        return None;
    }
    let dict = s.bits(8)?;
    if !(4..=6).contains(&dict) {
        return None;
    }
    let mut n = 0usize;
    while n < out.len() {
        if s.bits(1)? != 0 {
            let symbol = s.decode(&lencode)? as usize;
            let len = BASE[symbol] as usize + s.bits(EXTRA[symbol] as u32)? as usize;
            if len == 519 {
                break;
            }
            let shift = if len == 2 { 2 } else { dict };
            let dist = ((s.decode(&distcode)? as usize) << shift) + s.bits(shift)? as usize + 1;
            if dist > n {
                return None;
            }
            for _ in 0..len {
                if n >= out.len() {
                    break;
                }
                out[n] = out[n - dist];
                n += 1;
            }
        } else {
            let symbol = if lit != 0 {
                s.decode(&litcode)? as u32
            } else {
                s.bits(8)?
            };
            out[n] = symbol as u8;
            n += 1;
        }
    }
    Some(n)
}

#[cfg(test)]
mod tests {
    #[test]
    fn blast_reference_vector() {
        // From blast.c: "00 04 82 24 25 8f 80 7f" -> "AIAIAIAIAIAIA".
        let input = [0x00, 0x04, 0x82, 0x24, 0x25, 0x8f, 0x80, 0x7f];
        let mut out = [0u8; 64];
        let n = super::explode(&input, &mut out).unwrap();
        assert_eq!(&out[..n], b"AIAIAIAIAIAIA");
    }
}
