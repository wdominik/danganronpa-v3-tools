use crate::error::{BinError, BinResult};

/// Bounds-checked, position-tracking, endian-explicit reader over a byte
/// slice.
///
/// All multi-byte reads must be called with an explicit `_le` / `_be` suffix —
/// there is no default endianness because Danganronpa V3 formats mix the two.
#[derive(Debug, Clone)]
pub struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    /// Create a reader positioned at the start of `buf`.
    pub fn new(buf: &'a [u8]) -> Self {
        Self { buf, pos: 0 }
    }

    /// Current cursor offset from the start of the buffer.
    pub fn position(&self) -> usize {
        self.pos
    }

    /// Whether the cursor has reached the end of the buffer.
    pub fn is_empty(&self) -> bool {
        self.pos >= self.buf.len()
    }

    /// Number of bytes between the cursor and the end of the buffer.
    pub fn remaining(&self) -> usize {
        self.buf.len().saturating_sub(self.pos)
    }

    /// Whole underlying buffer (independent of `pos`).
    pub fn buffer(&self) -> &'a [u8] {
        self.buf
    }

    /// Borrow `buffer()[start..end]` with bounds and ordering checked.
    ///
    /// Unlike direct slice indexing, a malformed `start`/`end` returns an
    /// error instead of panicking. The returned slice borrows the underlying
    /// buffer (lifetime `'a`), independent of the cursor.
    ///
    /// # Errors
    ///
    /// Returns an error if `start > end` or `end` is past the end of the
    /// buffer.
    pub fn subslice(&self, start: usize, end: usize) -> BinResult<&'a [u8]> {
        if start > end || end > self.buf.len() {
            return Err(BinError::malformed(
                start,
                format!(
                    "subslice {start:#x}..{end:#x} out of bounds ({} bytes)",
                    self.buf.len()
                ),
            ));
        }
        Ok(&self.buf[start..end])
    }

    /// Capacity hint for a collection of `count` items, each at least
    /// `min_item_bytes` on disk, clamped to what the buffer can still supply.
    ///
    /// Pre-sizing a `Vec` directly from an untrusted `count` lets a malformed
    /// length field trigger a huge allocation before any bytes are read. This
    /// clamps the hint so the allocation can never exceed the remaining input;
    /// the per-item reads still fail with [`BinError::Eof`] if the data is
    /// actually truncated. Returns `0` when `min_item_bytes` is `0`.
    pub fn capacity_hint(&self, count: usize, min_item_bytes: usize) -> usize {
        if min_item_bytes == 0 {
            return 0;
        }
        count.min(self.remaining() / min_item_bytes)
    }

    /// Seek to an absolute offset.
    ///
    /// # Errors
    ///
    /// Returns an error if `pos` is past the end of the buffer.
    pub fn seek(&mut self, pos: usize) -> BinResult<()> {
        if pos > self.buf.len() {
            return Err(BinError::InvalidSeek {
                target: pos,
                capacity: self.buf.len(),
            });
        }
        self.pos = pos;
        Ok(())
    }

    /// Advance the cursor by `n` bytes.
    ///
    /// # Errors
    ///
    /// Returns an error if the advance would go past the end of the buffer.
    pub fn skip(&mut self, n: usize) -> BinResult<()> {
        let target = self.pos.checked_add(n).ok_or(BinError::InvalidSeek {
            target: usize::MAX,
            capacity: self.buf.len(),
        })?;
        self.seek(target)
    }

    /// Advance the cursor to the next multiple of `alignment`.
    ///
    /// # Errors
    ///
    /// Returns an error if the resulting position is past the end of the
    /// buffer.
    ///
    /// # Panics
    ///
    /// Panics (division by zero) if `alignment` is `0`. Every call site passes
    /// a non-zero literal; `alignment` is never derived from parsed input.
    pub fn align_to(&mut self, alignment: usize) -> BinResult<()> {
        debug_assert!(alignment != 0, "alignment must be non-zero");
        let padding = crate::align_up(self.pos, alignment) - self.pos;
        if padding != 0 {
            self.skip(padding)?;
        }
        Ok(())
    }

    /// Run a closure with the cursor temporarily moved to `pos`. Restores
    /// the original cursor on success and on error.
    ///
    /// # Errors
    ///
    /// Returns an error if `pos` is past the end of the buffer, or if the
    /// closure itself fails.
    pub fn with_seek<R>(
        &mut self,
        pos: usize,
        f: impl FnOnce(&mut Self) -> BinResult<R>,
    ) -> BinResult<R> {
        let saved = self.pos;
        self.seek(pos)?;
        let result = f(self);
        self.pos = saved;
        result
    }

    /// Read `n` bytes as a borrowed slice into the underlying buffer.
    ///
    /// # Errors
    ///
    /// Returns an error if fewer than `n` bytes remain in the buffer.
    pub fn bytes(&mut self, n: usize) -> BinResult<&'a [u8]> {
        // The returned slice borrows the underlying buffer (`'a`), not `self`,
        // so the immutable `peek_bytes` borrow ends before we advance the cursor.
        let slice = self.peek_bytes(n)?;
        self.pos += n;
        Ok(slice)
    }

    /// Peek `n` bytes without advancing.
    ///
    /// # Errors
    ///
    /// Returns an error if fewer than `n` bytes remain in the buffer.
    pub fn peek_bytes(&self, n: usize) -> BinResult<&'a [u8]> {
        let end = self.pos.checked_add(n).ok_or(BinError::Eof {
            pos: self.pos,
            wanted: n,
            remaining: self.remaining(),
        })?;
        if end > self.buf.len() {
            return Err(BinError::Eof {
                pos: self.pos,
                wanted: n,
                remaining: self.remaining(),
            });
        }
        Ok(&self.buf[self.pos..end])
    }

    /// Read `N` bytes into a fixed-size array.
    ///
    /// # Errors
    ///
    /// Returns an error if fewer than `N` bytes remain in the buffer.
    pub fn array<const N: usize>(&mut self) -> BinResult<[u8; N]> {
        let bytes = self.bytes(N)?;
        let mut out = [0u8; N];
        out.copy_from_slice(bytes);
        Ok(out)
    }

    /// Verify the next bytes equal `expected`; otherwise return [`BinError::BadMagic`].
    ///
    /// # Errors
    ///
    /// Returns [`BinError::BadMagic`] when the bytes at the cursor differ
    /// from `expected`, or [`BinError::Eof`] if fewer than `expected.len()`
    /// bytes remain.
    pub fn expect_magic(&mut self, expected: &[u8]) -> BinResult<()> {
        let pos = self.pos;
        let got = self.bytes(expected.len())?;
        if got != expected {
            return Err(BinError::BadMagic {
                pos,
                expected: expected.to_vec(),
                got: got.to_vec(),
            });
        }
        Ok(())
    }
}

macro_rules! read_int {
    ($name:ident, $ty:ty, $bytes:literal, $from:ident) => {
        impl Reader<'_> {
            /// Read a typed value at the cursor and advance by its size.
            ///
            /// # Errors
            ///
            /// Returns an error if fewer bytes remain than the type needs.
            pub fn $name(&mut self) -> BinResult<$ty> {
                let arr: [u8; $bytes] = self.array()?;
                Ok(<$ty>::$from(arr))
            }
        }
    };
}

read_int!(u8, u8, 1, from_le_bytes);
read_int!(i8, i8, 1, from_le_bytes);
read_int!(u16_le, u16, 2, from_le_bytes);
read_int!(u16_be, u16, 2, from_be_bytes);
read_int!(i16_le, i16, 2, from_le_bytes);
read_int!(i16_be, i16, 2, from_be_bytes);
read_int!(u32_le, u32, 4, from_le_bytes);
read_int!(u32_be, u32, 4, from_be_bytes);
read_int!(i32_le, i32, 4, from_le_bytes);
read_int!(i32_be, i32, 4, from_be_bytes);
read_int!(u64_le, u64, 8, from_le_bytes);
read_int!(u64_be, u64, 8, from_be_bytes);
read_int!(i64_le, i64, 8, from_le_bytes);
read_int!(i64_be, i64, 8, from_be_bytes);
read_int!(f32_le, f32, 4, from_le_bytes);
read_int!(f32_be, f32, 4, from_be_bytes);
read_int!(f64_le, f64, 8, from_le_bytes);
read_int!(f64_be, f64, 8, from_be_bytes);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_endian_explicit() {
        let mut r = Reader::new(&[0x01, 0x02, 0x03, 0x04]);
        assert_eq!(r.u32_le().unwrap(), 0x0403_0201);
        r.seek(0).unwrap();
        assert_eq!(r.u32_be().unwrap(), 0x0102_0304);
    }

    #[test]
    fn eof_reports_position_and_remaining() {
        let mut r = Reader::new(&[0x00, 0x01]);
        let err = r.u32_le().unwrap_err();
        match err {
            BinError::Eof {
                pos,
                wanted,
                remaining,
            } => {
                assert_eq!(pos, 0);
                assert_eq!(wanted, 4);
                assert_eq!(remaining, 2);
            }
            other => panic!("expected Eof, got {other:?}"),
        }
    }

    #[test]
    fn align_advances_to_next_boundary() {
        let mut r = Reader::new(&[0u8; 32]);
        r.skip(3).unwrap();
        r.align_to(16).unwrap();
        assert_eq!(r.position(), 16);
        r.align_to(16).unwrap();
        assert_eq!(r.position(), 16);
    }

    #[test]
    fn with_seek_restores_position() {
        let mut r = Reader::new(&[0u8; 16]);
        r.seek(4).unwrap();
        r.with_seek(10, |r| {
            assert_eq!(r.position(), 10);
            Ok(())
        })
        .unwrap();
        assert_eq!(r.position(), 4);
    }

    #[test]
    fn subslice_bounds_checked() {
        let r = Reader::new(&[0, 1, 2, 3, 4, 5, 6, 7]);
        assert_eq!(r.subslice(2, 5).unwrap(), &[2, 3, 4]);
        assert_eq!(r.subslice(4, 4).unwrap(), &[] as &[u8]);
        // end past the buffer and start > end both error rather than panic.
        assert!(matches!(r.subslice(4, 9), Err(BinError::Malformed { .. })));
        assert!(matches!(r.subslice(5, 2), Err(BinError::Malformed { .. })));
    }

    #[test]
    fn capacity_hint_clamps_to_remaining() {
        let mut r = Reader::new(&[0u8; 32]);
        // A hostile count is clamped to what the buffer could supply.
        assert_eq!(r.capacity_hint(1_000_000_000, 8), 4);
        // A count smaller than the ceiling passes through unchanged.
        assert_eq!(r.capacity_hint(3, 8), 3);
        // Zero item size never divides by zero.
        assert_eq!(r.capacity_hint(1_000, 0), 0);
        r.seek(32).unwrap();
        assert_eq!(r.capacity_hint(1_000, 8), 0);
    }

    #[test]
    fn expect_magic_matches_or_errors() {
        let mut r = Reader::new(b"CPK \x00");
        r.expect_magic(b"CPK ").unwrap();

        let mut r = Reader::new(b"NOPE");
        let err = r.expect_magic(b"CPK ").unwrap_err();
        assert!(matches!(err, BinError::BadMagic { .. }));
    }
}
