use std::str::Utf8Error;

use thiserror::Error;

/// Errors produced by `Reader` / `Writer` primitives.
///
/// Format-specific crates wrap or re-export this. The error always carries the
/// stream position so failures can be located in the source file without
/// re-parsing.
#[derive(Debug, Error)]
pub enum BinError {
    #[error(
        "unexpected end of input at offset {pos:#x}: wanted {wanted} byte(s), have {remaining}"
    )]
    Eof {
        pos: usize,
        wanted: usize,
        remaining: usize,
    },

    #[error("bad magic at offset {pos:#x}: expected {expected:02x?}, got {got:02x?}")]
    BadMagic {
        pos: usize,
        expected: Vec<u8>,
        got: Vec<u8>,
    },

    #[error("invalid seek to offset {target:#x}: capacity is {capacity:#x}")]
    InvalidSeek { target: usize, capacity: usize },

    #[error("invalid UTF-16 LE string starting at offset {pos:#x}")]
    InvalidUtf16 { pos: usize },

    #[error("invalid UTF-8 string at offset {pos:#x}")]
    InvalidUtf8 {
        pos: usize,
        #[source]
        source: Utf8Error,
    },

    #[error("unterminated string starting at offset {pos:#x}")]
    UnterminatedString { pos: usize },

    #[error("malformed data at offset {pos:#x}: {message}")]
    Malformed { pos: usize, message: String },
}

impl BinError {
    /// Build a [`BinError::Malformed`] at the given position.
    pub fn malformed(pos: usize, message: impl Into<String>) -> Self {
        Self::Malformed {
            pos,
            message: message.into(),
        }
    }
}

pub type BinResult<T> = Result<T, BinError>;
