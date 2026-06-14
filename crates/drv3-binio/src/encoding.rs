//! String encoding helpers for the three conventions used across the DR V3
//! format family:
//!
//! - **UTF-16 LE, null-terminated** (one `u16 == 0` terminator): STX dialogue
//!   text, DAT `utf16` cells, `SpFt` font names.
//! - **UTF-8, null-terminated** (single `0x00` terminator): DAT column names
//!   and DAT `ascii` / `label` / `refer` cells.
//! - **Pascal-style** (`u8 length` byte, then `length` bytes of payload, then
//!   a `0x00` terminator): WRD labels and parameters.

use crate::{
    error::{BinError, BinResult},
    reader::Reader,
    writer::Writer,
};

impl Reader<'_> {
    /// Read a UTF-16 LE null-terminated string. Consumes the terminator.
    ///
    /// # Errors
    ///
    /// Returns an error if the buffer ends before the null terminator is
    /// reached, or if the UTF-16 code units don't form valid Unicode.
    pub fn read_utf16le_cstring(&mut self) -> BinResult<String> {
        let start = self.position();
        let mut units: Vec<u16> = Vec::new();
        loop {
            let unit = self
                .u16_le()
                .map_err(|_| BinError::UnterminatedString { pos: start })?;
            if unit == 0 {
                break;
            }
            units.push(unit);
        }
        String::from_utf16(&units).map_err(|_| BinError::InvalidUtf16 { pos: start })
    }

    /// Read a UTF-16 LE string of an exact byte length (no terminator).
    ///
    /// # Errors
    ///
    /// Returns an error if `byte_len` is odd, if the read runs past the
    /// buffer, or if the bytes don't form valid UTF-16.
    pub fn read_utf16le_exact(&mut self, byte_len: usize) -> BinResult<String> {
        let start = self.position();
        if byte_len % 2 != 0 {
            return Err(BinError::malformed(
                start,
                format!("UTF-16 LE byte length must be even, got {byte_len}"),
            ));
        }
        let bytes = self.bytes(byte_len)?;
        let units: Vec<u16> = bytes
            .chunks_exact(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .collect();
        String::from_utf16(&units).map_err(|_| BinError::InvalidUtf16 { pos: start })
    }

    /// Read a UTF-8 null-terminated string. Consumes the terminator.
    ///
    /// # Errors
    ///
    /// Returns an error if the buffer ends before the null terminator is
    /// reached, or if the bytes are not valid UTF-8.
    pub fn read_utf8_cstring(&mut self) -> BinResult<String> {
        let start = self.position();
        let mut bytes: Vec<u8> = Vec::new();
        loop {
            let b = self
                .u8()
                .map_err(|_| BinError::UnterminatedString { pos: start })?;
            if b == 0 {
                break;
            }
            bytes.push(b);
        }
        std::str::from_utf8(&bytes)
            .map(str::to_owned)
            .map_err(|source| BinError::InvalidUtf8 { pos: start, source })
    }

    /// Read a UTF-8 string of exact byte length (no terminator).
    ///
    /// # Errors
    ///
    /// Returns an error if the read runs past the buffer or the bytes are
    /// not valid UTF-8.
    pub fn read_utf8_exact(&mut self, byte_len: usize) -> BinResult<String> {
        let start = self.position();
        let bytes = self.bytes(byte_len)?;
        std::str::from_utf8(bytes)
            .map(str::to_owned)
            .map_err(|source| BinError::InvalidUtf8 { pos: start, source })
    }

    /// Read a WRD-style Pascal string: `u8 length, length bytes, 0x00 terminator`.
    /// The terminator is mandatory and consumed.
    ///
    /// # Errors
    ///
    /// Returns an error if the read runs past the buffer, the terminator
    /// byte is non-zero, or the payload bytes are not valid UTF-8.
    pub fn read_pascal_string(&mut self) -> BinResult<String> {
        let start = self.position();
        let len = self.u8()? as usize;
        let bytes = self.bytes(len)?;
        let terminator = self.u8()?;
        if terminator != 0 {
            return Err(BinError::malformed(
                start,
                format!("pascal string missing null terminator (got {terminator:#x})"),
            ));
        }
        std::str::from_utf8(bytes)
            .map(str::to_owned)
            .map_err(|source| BinError::InvalidUtf8 { pos: start, source })
    }

    /// Read a UTF-8 cstring from an absolute position without disturbing
    /// the cursor (used for string pools that store offsets).
    ///
    /// # Errors
    ///
    /// Returns an error if `pos` is past the buffer end, the buffer ends
    /// before the null terminator, or the bytes are not valid UTF-8.
    pub fn read_utf8_cstring_at(&mut self, pos: usize) -> BinResult<String> {
        self.with_seek(pos, Reader::read_utf8_cstring)
    }
}

impl Writer {
    /// Encode a UTF-16 LE null-terminated string (terminator included).
    pub fn write_utf16le_cstring(&mut self, s: &str) {
        for unit in s.encode_utf16() {
            self.write_u16_le(unit);
        }
        self.write_u16_le(0);
    }

    /// Encode a UTF-16 LE string with no terminator.
    pub fn write_utf16le_raw(&mut self, s: &str) {
        for unit in s.encode_utf16() {
            self.write_u16_le(unit);
        }
    }

    /// Encode a UTF-8 null-terminated string (terminator included).
    pub fn write_utf8_cstring(&mut self, s: &str) {
        self.write_bytes(s.as_bytes());
        self.write_u8(0);
    }

    /// Encode a WRD-style Pascal string.
    ///
    /// # Errors
    ///
    /// Returns an error if `s` is longer than 255 bytes (the Pascal length
    /// prefix is a single byte).
    pub fn write_pascal_string(&mut self, s: &str) -> BinResult<()> {
        let bytes = s.as_bytes();
        if bytes.len() > u8::MAX as usize {
            return Err(BinError::malformed(
                self.position(),
                format!("pascal string of {} bytes exceeds u8 length", bytes.len()),
            ));
        }
        self.write_u8(bytes.len() as u8);
        self.write_bytes(bytes);
        self.write_u8(0);
        Ok(())
    }
}

/// Byte length of a UTF-16 LE encoded string (no terminator).
#[must_use]
pub fn utf16le_byte_len(s: &str) -> usize {
    s.encode_utf16().count() * 2
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utf16_round_trip() {
        let mut w = Writer::new();
        w.write_utf16le_cstring("Hi 😀");
        let mut r = Reader::new(w.buffer());
        assert_eq!(r.read_utf16le_cstring().unwrap(), "Hi 😀");
    }

    #[test]
    fn utf8_cstring_round_trip() {
        let mut w = Writer::new();
        w.write_utf8_cstring("hello");
        assert_eq!(w.buffer(), b"hello\0");
        let mut r = Reader::new(w.buffer());
        assert_eq!(r.read_utf8_cstring().unwrap(), "hello");
    }

    #[test]
    fn pascal_string_round_trip() {
        let mut w = Writer::new();
        w.write_pascal_string("label").unwrap();
        assert_eq!(w.buffer(), b"\x05label\0");
        let mut r = Reader::new(w.buffer());
        assert_eq!(r.read_pascal_string().unwrap(), "label");
    }

    #[test]
    fn unterminated_utf16_errors() {
        let mut r = Reader::new(&[0x48, 0x00, 0x69, 0x00]);
        let err = r.read_utf16le_cstring().unwrap_err();
        assert!(matches!(err, BinError::UnterminatedString { .. }));
    }

    #[test]
    fn utf16_byte_len_matches_encoded() {
        assert_eq!(utf16le_byte_len("Hi"), 4);
        assert_eq!(utf16le_byte_len("😀"), 4); // surrogate pair
    }
}
