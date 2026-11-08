//! Little-endian binary reading/writing helpers for the SC2 binary components.

use crate::FormatError;

pub(crate) struct Reader<'a> {
    pub data: &'a [u8],
    pub pos: usize,
    file: &'static str,
}

impl<'a> Reader<'a> {
    pub fn new(data: &'a [u8], file: &'static str) -> Self {
        Self { data, pos: 0, file }
    }

    pub fn err(&self, reason: impl Into<String>) -> FormatError {
        FormatError::Malformed { file: self.file.to_owned(), reason: reason.into() }
    }

    pub fn bytes(&mut self, n: usize) -> Result<&'a [u8], FormatError> {
        if self.pos + n > self.data.len() {
            return Err(self.err(format!("ended unexpectedly at byte {}", self.pos)));
        }
        let s = &self.data[self.pos..self.pos + n];
        self.pos += n;
        Ok(s)
    }

    pub fn skip(&mut self, n: usize) -> Result<(), FormatError> {
        self.bytes(n).map(|_| ())
    }

    pub fn u8(&mut self) -> Result<u8, FormatError> {
        Ok(self.bytes(1)?[0])
    }

    pub fn u16(&mut self) -> Result<u16, FormatError> {
        let b = self.bytes(2)?;
        Ok(u16::from_le_bytes([b[0], b[1]]))
    }

    pub fn u32(&mut self) -> Result<u32, FormatError> {
        let b = self.bytes(4)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    pub fn i32(&mut self) -> Result<i32, FormatError> {
        Ok(self.u32()? as i32)
    }

    /// NUL-terminated string (lossy UTF-8).
    pub fn cstr(&mut self) -> Result<String, FormatError> {
        let rest = &self.data[self.pos..];
        let end = rest.iter().position(|&b| b == 0).ok_or_else(|| self.err("unterminated string"))?;
        let s = String::from_utf8_lossy(&rest[..end]).into_owned();
        self.pos += end + 1;
        Ok(s)
    }

    /// u16-length-prefixed string.
    pub fn lpstr(&mut self) -> Result<String, FormatError> {
        let n = self.u16()? as usize;
        Ok(String::from_utf8_lossy(self.bytes(n)?).into_owned())
    }
}

pub(crate) trait Put {
    fn put_u16(&mut self, v: u16);
    fn put_u32(&mut self, v: u32);
    fn put_i32(&mut self, v: i32);
    fn put_f32(&mut self, v: f32);
    fn put_cstr(&mut self, s: &str);
    fn put_lpstr(&mut self, s: &str);
}

impl Put for Vec<u8> {
    fn put_u16(&mut self, v: u16) {
        self.extend_from_slice(&v.to_le_bytes());
    }
    fn put_u32(&mut self, v: u32) {
        self.extend_from_slice(&v.to_le_bytes());
    }
    fn put_i32(&mut self, v: i32) {
        self.extend_from_slice(&v.to_le_bytes());
    }
    fn put_f32(&mut self, v: f32) {
        self.extend_from_slice(&v.to_le_bytes());
    }
    fn put_cstr(&mut self, s: &str) {
        self.extend_from_slice(s.as_bytes());
        self.push(0);
    }
    fn put_lpstr(&mut self, s: &str) {
        self.put_u16(s.len() as u16);
        self.extend_from_slice(s.as_bytes());
    }
}

pub(crate) fn read_u16(d: &[u8], off: usize) -> u16 {
    u16::from_le_bytes([d[off], d[off + 1]])
}

pub(crate) fn read_u32(d: &[u8], off: usize) -> u32 {
    u32::from_le_bytes([d[off], d[off + 1], d[off + 2], d[off + 3]])
}

pub(crate) fn write_u16(d: &mut [u8], off: usize, v: u16) {
    d[off..off + 2].copy_from_slice(&v.to_le_bytes());
}
