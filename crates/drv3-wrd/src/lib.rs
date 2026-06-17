//! Danganronpa V3 WRD script reader/writer.
//!
//! WRD is a bytecode-script container paired with an STX file: opcodes such
//! as `LOC` reference STX string IDs and `CHN`/`CHR` provide speaker
//! metadata. The on-disk layout uses a split-endian header (LE counts and
//! offsets) followed by a big-endian opcode stream:
//!
//! ```text
//! offset 0x00   2 bytes   string_count u16 LE
//! offset 0x02   2 bytes   label_count  u16 LE
//! offset 0x04   2 bytes   parameter_count u16 LE
//! offset 0x06   2 bytes   local_branch_count u16 LE
//! offset 0x08   4 bytes   unknown1 u32 LE — preserved verbatim
//! offset 0x0C   4 bytes   local_branch_data_ptr u32 LE
//! offset 0x10   4 bytes   label_offsets_ptr u32 LE
//! offset 0x14   4 bytes   label_names_ptr u32 LE
//! offset 0x18   4 bytes   parameters_ptr u32 LE
//! offset 0x1C   4 bytes   strings_ptr u32 LE — 0 when strings live in the paired STX
//! offset 0x20   …         bytecode stream: bytes alternating between opcode
//!                         tags (often prefixed by 0x70) and u16 BE arguments
//! …             …         local-branch table, label offsets, label names,
//!                         parameters, optional internal-strings table
//! ```
//!
//! Label names and parameters are Pascal-style strings (`u8 length, bytes,
//! 0x00`). Numeric arguments in the bytecode are *big-endian* even though
//! the header is little-endian — both endians are explicit at every read.
//!
//! ## Status
//!
//! Translation tooling never *modifies* WRD files — they are read for speaker
//! and choice context, and copied through unchanged. We implement a symmetric
//! reader/writer anyway so [`drv3-cli`](../drv3_cli/index.html)'s `roundtrip`
//! subcommand can validate WRD bytes byte-for-byte.
//!
//! The byte-code stream is parsed *structurally*: each [`Command`] keeps its
//! opcode byte and its raw u16 (big-endian) arguments. The 76-entry
//! argument-type table from Harmony-Tools is **not** required for structural
//! round-trip and is not embedded; consumers that want decoded argument
//! semantics can use [`Wrd::iter_dialogue_lines`] for the LOC/CHN/CHR/CHK
//! subset, or interpret raw arguments themselves.

use drv3_binio::{BinError, BinResult, Reader, Writer};

const HEADER_SIZE: u32 = 0x20;
const COMMAND_MARKER: u8 = 0x70;

/// Opcode index for `CHK` (branch / choice metadata).
pub const OPCODE_CHK: u8 = 0x0A;
/// Opcode index for `LAB` (mark a label).
pub const OPCODE_LAB: u8 = 0x14;
/// Opcode index for `CHN` (set the currently-speaking character).
pub const OPCODE_CHN: u8 = 0x1D;
/// Opcode index for `CHR` (character parameters; sometimes synthesised into a `CHN`).
pub const OPCODE_CHR: u8 = 0x22;
/// Opcode index for `LOC` (display a string from the paired STX).
pub const OPCODE_LOC: u8 = 0x4B;

/// Parsed WRD script file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Wrd {
    /// 4-byte field at header offset 0x08 — preserve verbatim.
    pub unknown1: u32,
    /// Byte-code commands in stream order.
    pub commands: Vec<Command>,
    /// Local-branch table.
    pub local_branches: Vec<LocalBranch>,
    /// Byte-code offsets of each `LAB` opcode, in label-index order.
    pub label_offsets: Vec<u16>,
    /// Label names (pascal-string-encoded on disk).
    pub label_names: Vec<String>,
    /// Parameter names (pascal-string-encoded on disk).
    pub parameters: Vec<String>,
    /// Internal dialogue strings (UTF-16 LE). `None` means the WRD references
    /// the paired STX file for dialogue (the common DR V3 case).
    pub internal_strings: Option<Vec<String>>,
    /// `StringCount` from the header — preserved separately so external-string
    /// files round-trip even though no strings appear inline.
    pub external_string_count: u16,
}

/// A single byte-code command.
///
/// Arguments are stored verbatim as 16-bit big-endian values; consult the
/// opcode-specific argument-type table (Harmony-Tools `WrdCommandHelper`) to
/// interpret them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Command {
    pub opcode: u8,
    pub args: Vec<u16>,
}

/// One row of the local-branch table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LocalBranch {
    pub id: u16,
    /// Byte-code offset of the corresponding `LBN`.
    pub offset: u16,
}

/// A speaker/text pair recovered by [`Wrd::iter_dialogue_lines`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DialogueLine {
    /// Index into [`Wrd::parameters`] for the most recent `CHN`/`CHR`, or `None`
    /// if no speaker was set.
    pub speaker_param: Option<u16>,
    /// STX `StringId` of the `LOC` opcode.
    pub string_id: u16,
}

impl Wrd {
    /// Parse a WRD file from a byte buffer.
    ///
    /// # Errors
    ///
    /// Returns an error if the header counts/offsets don't fit the
    /// buffer, the bytecode stream is truncated, a label/parameter
    /// Pascal string is malformed, or a declared offset points past the
    /// end of its section.
    pub fn parse(input: &[u8]) -> BinResult<Self> {
        let mut r = Reader::new(input);

        let string_count = r.u16_le()?;
        let label_count = r.u16_le()? as usize;
        let parameter_count = r.u16_le()? as usize;
        let local_branch_count = r.u16_le()? as usize;
        let unknown1 = r.u32_le()?;
        let local_branch_data_ptr = r.u32_le()? as usize;
        let label_offsets_ptr = r.u32_le()? as usize;
        let label_names_ptr = r.u32_le()? as usize;
        let parameters_ptr = r.u32_le()? as usize;
        let strings_ptr = r.u32_le()? as usize;

        // Byte-code stream.
        r.seek(HEADER_SIZE as usize)?;
        let commands = parse_byte_code(&mut r, local_branch_data_ptr)?;

        // Local branches.
        r.seek(local_branch_data_ptr)?;
        let mut local_branches: Vec<LocalBranch> = Vec::with_capacity(local_branch_count);
        for _ in 0..local_branch_count {
            let id = r.u16_le()?;
            let offset = r.u16_le()?;
            local_branches.push(LocalBranch { id, offset });
        }

        // Label offsets.
        r.seek(label_offsets_ptr)?;
        let mut label_offsets: Vec<u16> = Vec::with_capacity(label_count);
        for _ in 0..label_count {
            label_offsets.push(r.u16_le()?);
        }

        // Label names.
        r.seek(label_names_ptr)?;
        let mut label_names: Vec<String> = Vec::with_capacity(label_count);
        for _ in 0..label_count {
            label_names.push(r.read_pascal_string()?);
        }

        // Parameters.
        r.seek(parameters_ptr)?;
        let mut parameters: Vec<String> = Vec::with_capacity(parameter_count);
        for _ in 0..parameter_count {
            parameters.push(r.read_pascal_string()?);
        }

        // Internal strings (optional).
        let internal_strings = if strings_ptr == 0 {
            None
        } else {
            r.seek(strings_ptr)?;
            let mut strings: Vec<String> = Vec::with_capacity(string_count as usize);
            for _ in 0..string_count {
                strings.push(r.read_utf16le_cstring()?);
            }
            Some(strings)
        };

        Ok(Self {
            unknown1,
            commands,
            local_branches,
            label_offsets,
            label_names,
            parameters,
            internal_strings,
            external_string_count: string_count,
        })
    }

    /// Encode a WRD file to a byte vector.
    ///
    /// # Errors
    ///
    /// Returns an error if `label_offsets` and `label_names` have
    /// different lengths, any label or parameter exceeds the Pascal-
    /// string 255-byte limit, or a count overflows its on-disk width.
    pub fn to_bytes(&self) -> BinResult<Vec<u8>> {
        // Validate that auxiliary tables agree on their counts.
        if self.label_names.len() != self.label_offsets.len() {
            return Err(BinError::malformed(
                0,
                format!(
                    "label_names ({}) and label_offsets ({}) must match in length",
                    self.label_names.len(),
                    self.label_offsets.len()
                ),
            ));
        }
        if self.label_names.len() > u16::MAX as usize
            || self.parameters.len() > u16::MAX as usize
            || self.local_branches.len() > u16::MAX as usize
        {
            return Err(BinError::malformed(0, "auxiliary table count exceeds u16"));
        }

        // Plan: write header placeholder, then byte-code, then aux sections,
        // then patch header offsets.
        let mut w = Writer::new();

        // Reserve header.
        w.write_fill(HEADER_SIZE as usize, 0);

        // Byte-code stream.
        for cmd in &self.commands {
            w.write_u8(COMMAND_MARKER);
            w.write_u8(cmd.opcode);
            for &arg in &cmd.args {
                w.write_u16_be(arg);
            }
        }

        // Local branches.
        let local_branch_data_ptr = w.position() as u32;
        for branch in &self.local_branches {
            w.write_u16_le(branch.id);
            w.write_u16_le(branch.offset);
        }

        // Label offsets.
        let label_offsets_ptr = w.position() as u32;
        for &off in &self.label_offsets {
            w.write_u16_le(off);
        }

        // Label names.
        let label_names_ptr = w.position() as u32;
        for name in &self.label_names {
            w.write_pascal_string(name)?;
        }

        // Parameters.
        let parameters_ptr = w.position() as u32;
        for name in &self.parameters {
            w.write_pascal_string(name)?;
        }

        // Internal strings (optional).
        let (string_count, strings_ptr) = match &self.internal_strings {
            None => (self.external_string_count, 0u32),
            Some(strings) => {
                let pos = w.position() as u32;
                for s in strings {
                    w.write_utf16le_cstring(s);
                }
                if strings.len() > u16::MAX as usize {
                    return Err(BinError::malformed(0, "internal_strings count exceeds u16"));
                }
                (strings.len() as u16, pos)
            }
        };

        // Patch header.
        let mut bytes = w.into_inner();
        bytes[0x00..0x02].copy_from_slice(&string_count.to_le_bytes());
        bytes[0x02..0x04].copy_from_slice(&(self.label_names.len() as u16).to_le_bytes());
        bytes[0x04..0x06].copy_from_slice(&(self.parameters.len() as u16).to_le_bytes());
        bytes[0x06..0x08].copy_from_slice(&(self.local_branches.len() as u16).to_le_bytes());
        bytes[0x08..0x0C].copy_from_slice(&self.unknown1.to_le_bytes());
        bytes[0x0C..0x10].copy_from_slice(&local_branch_data_ptr.to_le_bytes());
        bytes[0x10..0x14].copy_from_slice(&label_offsets_ptr.to_le_bytes());
        bytes[0x14..0x18].copy_from_slice(&label_names_ptr.to_le_bytes());
        bytes[0x18..0x1C].copy_from_slice(&parameters_ptr.to_le_bytes());
        bytes[0x1C..0x20].copy_from_slice(&strings_ptr.to_le_bytes());

        Ok(bytes)
    }

    /// Walk the byte-code stream and yield `(speaker, string_id)` pairs for each
    /// `LOC` opcode encountered. The speaker is the most recent `CHN` or `CHR`
    /// argument before the `LOC`, or `None` if no speaker has been set.
    ///
    /// This is the primary API for the translation tool: given a WRD and its
    /// paired STX, it produces the speaker context for every translatable line.
    pub fn iter_dialogue_lines(&self) -> impl Iterator<Item = DialogueLine> + '_ {
        let mut speaker: Option<u16> = None;
        self.commands
            .iter()
            .filter_map(move |cmd| match cmd.opcode {
                OPCODE_CHN | OPCODE_CHR => {
                    speaker = cmd.args.first().copied();
                    None
                }
                OPCODE_LOC => cmd.args.first().map(|&id| DialogueLine {
                    speaker_param: speaker,
                    string_id: id,
                }),
                _ => None,
            })
    }
}

fn parse_byte_code(r: &mut Reader<'_>, end: usize) -> BinResult<Vec<Command>> {
    let mut commands = Vec::new();
    while r.position() < end {
        let pos = r.position();
        let marker = r.u8()?;
        if marker != COMMAND_MARKER {
            return Err(BinError::malformed(
                pos,
                format!("expected {COMMAND_MARKER:#x} command marker, got {marker:#x}"),
            ));
        }
        let opcode = r.u8()?;
        let mut args = Vec::new();
        loop {
            // Need at least two bytes for an arg, AND the next byte must not
            // be the next command's marker.
            if r.position() + 2 > end {
                break;
            }
            let peek = r.peek_bytes(1)?[0];
            if peek == COMMAND_MARKER {
                break;
            }
            let hi = r.u8()?;
            let lo = r.u8()?;
            args.push((u16::from(hi) << 8) | u16::from(lo));
        }
        commands.push(Command { opcode, args });
    }
    Ok(commands)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Wrd {
        Wrd {
            unknown1: 0xCAFE_BABE,
            commands: vec![
                Command {
                    opcode: OPCODE_LAB,
                    args: vec![0x0000],
                },
                Command {
                    opcode: OPCODE_CHN,
                    args: vec![0x0003],
                }, // speaker = parameter 3
                Command {
                    opcode: OPCODE_LOC,
                    args: vec![0x0001],
                }, // STX id 1
                Command {
                    opcode: OPCODE_LOC,
                    args: vec![0x0002],
                }, // STX id 2, same speaker
                Command {
                    opcode: OPCODE_CHR,
                    args: vec![0x0004, 0x0005],
                }, // speaker = parameter 4
                Command {
                    opcode: OPCODE_LOC,
                    args: vec![0x0003],
                }, // STX id 3
                Command {
                    opcode: OPCODE_CHK,
                    args: vec![0x0006],
                },
            ],
            local_branches: vec![LocalBranch { id: 1, offset: 16 }],
            label_offsets: vec![0],
            label_names: vec!["start".into()],
            parameters: vec!["", "param1", "param2", "Alice", "Bob", "extra", "choice_a"]
                .into_iter()
                .map(String::from)
                .collect(),
            internal_strings: None,
            external_string_count: 3,
        }
    }

    #[test]
    fn round_trip_preserves_bytes() {
        let wrd = sample();
        let bytes = wrd.to_bytes().unwrap();
        let parsed = Wrd::parse(&bytes).unwrap();
        assert_eq!(parsed, wrd);
        assert_eq!(parsed.to_bytes().unwrap(), bytes);
    }

    #[test]
    fn dialogue_iterator_associates_speakers() {
        let wrd = sample();
        let lines: Vec<DialogueLine> = wrd.iter_dialogue_lines().collect();
        assert_eq!(
            lines,
            vec![
                DialogueLine {
                    speaker_param: Some(3),
                    string_id: 1
                },
                DialogueLine {
                    speaker_param: Some(3),
                    string_id: 2
                },
                DialogueLine {
                    speaker_param: Some(4),
                    string_id: 3
                },
            ]
        );
    }

    #[test]
    fn opcodes_without_args_are_supported() {
        let wrd = Wrd {
            unknown1: 0,
            commands: vec![
                Command {
                    opcode: 0x00,
                    args: vec![],
                }, // no-arg opcode
                Command {
                    opcode: 0x01,
                    args: vec![],
                },
            ],
            local_branches: vec![],
            label_offsets: vec![],
            label_names: vec![],
            parameters: vec![],
            internal_strings: None,
            external_string_count: 0,
        };
        let bytes = wrd.to_bytes().unwrap();
        let parsed = Wrd::parse(&bytes).unwrap();
        assert_eq!(parsed, wrd);
    }

    #[test]
    fn internal_strings_round_trip() {
        let wrd = Wrd {
            unknown1: 1,
            commands: vec![Command {
                opcode: OPCODE_LOC,
                args: vec![0],
            }],
            local_branches: vec![],
            label_offsets: vec![],
            label_names: vec![],
            parameters: vec![],
            internal_strings: Some(vec!["Inline string!".into(), "日本語".into()]),
            external_string_count: 2, // ignored since internal_strings is Some
        };
        let bytes = wrd.to_bytes().unwrap();
        let parsed = Wrd::parse(&bytes).unwrap();
        assert_eq!(parsed.internal_strings, wrd.internal_strings);
    }

    #[test]
    fn rejects_missing_command_marker() {
        // Build a wrd, then corrupt the first byte-code byte.
        let mut bytes = sample().to_bytes().unwrap();
        bytes[HEADER_SIZE as usize] = 0x71; // not 0x70
        let err = Wrd::parse(&bytes).unwrap_err();
        assert!(matches!(err, BinError::Malformed { .. }));
    }
}
