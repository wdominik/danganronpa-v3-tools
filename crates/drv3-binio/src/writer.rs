use crate::error::{BinError, BinResult};

/// Owning writer that appends to a [`Vec<u8>`] with endian-explicit accessors
/// and back-patching support.
///
/// Most format writers compute offsets in a second pass; [`Writer::reserve_u32_le`]
/// returns a [`Patch`] handle that can be filled in later via
/// [`Writer::patch_u32_le`].
#[derive(Debug, Default)]
pub struct Writer {
    buf: Vec<u8>,
}

/// Handle to a placeholder slot reserved earlier in the stream.
#[derive(Debug, Clone, Copy)]
#[must_use = "patches must be filled in with Writer::patch_*"]
pub struct Patch {
    pos: usize,
    len: usize,
}

impl Writer {
    /// Create an empty writer.
    pub fn new() -> Self {
        Self::default()
    }

    /// Create an empty writer with room pre-allocated for `cap` bytes.
    pub fn with_capacity(cap: usize) -> Self {
        Self {
            buf: Vec::with_capacity(cap),
        }
    }

    /// Current length of the stream — also the offset the next write lands at.
    pub fn position(&self) -> usize {
        self.buf.len()
    }

    /// Borrow the bytes written so far.
    pub fn buffer(&self) -> &[u8] {
        &self.buf
    }

    /// Consume the writer and return the underlying byte buffer.
    pub fn into_inner(self) -> Vec<u8> {
        self.buf
    }

    /// Append raw bytes to the end of the stream.
    pub fn write_bytes(&mut self, data: &[u8]) {
        self.buf.extend_from_slice(data);
    }

    /// Append `count` copies of `fill`.
    pub fn write_fill(&mut self, count: usize, fill: u8) {
        self.buf.resize(self.buf.len() + count, fill);
    }

    /// Pad the stream up to the next multiple of `alignment` with `fill`.
    ///
    /// # Panics
    ///
    /// Panics (division by zero) if `alignment` is `0`. Every call site passes
    /// a non-zero literal.
    pub fn pad_to(&mut self, alignment: usize, fill: u8) {
        debug_assert!(alignment != 0, "alignment must be non-zero");
        let padding = crate::align_up(self.buf.len(), alignment) - self.buf.len();
        self.write_fill(padding, fill);
    }

    /// Reserve `len` bytes (zero-filled) and return a [`Patch`] handle.
    pub fn reserve(&mut self, len: usize) -> Patch {
        let pos = self.buf.len();
        self.write_fill(len, 0);
        Patch { pos, len }
    }

    /// Reserve a 4-byte little-endian slot and return its [`Patch`] handle.
    pub fn reserve_u32_le(&mut self) -> Patch {
        self.reserve(4)
    }

    /// Overwrite a previously reserved 4-byte slot with a little-endian u32.
    ///
    /// # Errors
    ///
    /// Returns an error if `patch` does not refer to a 4-byte slot — i.e.
    /// the [`Patch`] was created by a different `reserve_*` size.
    pub fn patch_u32_le(&mut self, patch: Patch, value: u32) -> BinResult<()> {
        self.patch_slice(patch, &value.to_le_bytes())
    }

    /// Overwrite a previously reserved 4-byte slot with a big-endian u32.
    ///
    /// # Errors
    ///
    /// Returns an error if `patch` does not refer to a 4-byte slot.
    pub fn patch_u32_be(&mut self, patch: Patch, value: u32) -> BinResult<()> {
        self.patch_slice(patch, &value.to_be_bytes())
    }

    /// Overwrite a previously reserved 2-byte slot with a little-endian u16.
    ///
    /// # Errors
    ///
    /// Returns an error if `patch` does not refer to a 2-byte slot.
    pub fn patch_u16_le(&mut self, patch: Patch, value: u16) -> BinResult<()> {
        self.patch_slice(patch, &value.to_le_bytes())
    }

    /// Overwrite a previously reserved 2-byte slot with a big-endian u16.
    ///
    /// # Errors
    ///
    /// Returns an error if `patch` does not refer to a 2-byte slot.
    pub fn patch_u16_be(&mut self, patch: Patch, value: u16) -> BinResult<()> {
        self.patch_slice(patch, &value.to_be_bytes())
    }

    /// Overwrite a previously reserved slot with `data`.
    ///
    /// # Errors
    ///
    /// Returns an error if `data.len()` differs from the reserved slot's size.
    pub fn patch_slice(&mut self, patch: Patch, data: &[u8]) -> BinResult<()> {
        if patch.len != data.len() {
            return Err(BinError::malformed(
                patch.pos,
                format!(
                    "patch size mismatch: reserved {} bytes, wrote {}",
                    patch.len,
                    data.len()
                ),
            ));
        }
        let end = patch.pos + patch.len;
        if end > self.buf.len() {
            return Err(BinError::InvalidSeek {
                target: end,
                capacity: self.buf.len(),
            });
        }
        self.buf[patch.pos..end].copy_from_slice(data);
        Ok(())
    }
}

macro_rules! write_int {
    ($name:ident, $ty:ty, $method:ident) => {
        impl Writer {
            /// Append a typed value to the stream in the byte order named by
            /// the method (`_le` / `_be`).
            pub fn $name(&mut self, value: $ty) {
                self.write_bytes(&value.$method());
            }
        }
    };
}

write_int!(write_u8, u8, to_le_bytes);
write_int!(write_i8, i8, to_le_bytes);
write_int!(write_u16_le, u16, to_le_bytes);
write_int!(write_u16_be, u16, to_be_bytes);
write_int!(write_i16_le, i16, to_le_bytes);
write_int!(write_i16_be, i16, to_be_bytes);
write_int!(write_u32_le, u32, to_le_bytes);
write_int!(write_u32_be, u32, to_be_bytes);
write_int!(write_i32_le, i32, to_le_bytes);
write_int!(write_i32_be, i32, to_be_bytes);
write_int!(write_u64_le, u64, to_le_bytes);
write_int!(write_u64_be, u64, to_be_bytes);
write_int!(write_i64_le, i64, to_le_bytes);
write_int!(write_i64_be, i64, to_be_bytes);
write_int!(write_f32_le, f32, to_le_bytes);
write_int!(write_f32_be, f32, to_be_bytes);
write_int!(write_f64_le, f64, to_le_bytes);
write_int!(write_f64_be, f64, to_be_bytes);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_endian_explicit() {
        let mut w = Writer::new();
        w.write_u32_le(0x0403_0201);
        w.write_u32_be(0x0102_0304);
        assert_eq!(
            w.buffer(),
            &[0x01, 0x02, 0x03, 0x04, 0x01, 0x02, 0x03, 0x04]
        );
    }

    #[test]
    fn pad_to_rounds_up() {
        let mut w = Writer::new();
        w.write_bytes(&[1, 2, 3]);
        w.pad_to(16, 0);
        assert_eq!(w.position(), 16);
        w.pad_to(16, 0);
        assert_eq!(w.position(), 16);
    }

    #[test]
    fn back_patch_offset() {
        let mut w = Writer::new();
        let patch = w.reserve_u32_le();
        w.write_bytes(b"hello");
        let offset = w.position() as u32;
        w.patch_u32_le(patch, offset).unwrap();
        assert_eq!(&w.buffer()[..4], &9u32.to_le_bytes());
    }

    #[test]
    fn back_patch_size_mismatch_errors() {
        let mut w = Writer::new();
        let patch = w.reserve(2);
        let err = w.patch_slice(patch, &[1, 2, 3]).unwrap_err();
        assert!(matches!(err, BinError::Malformed { .. }));
    }
}
