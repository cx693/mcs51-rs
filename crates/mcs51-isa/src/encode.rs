//! Instruction encoding: [`Instruction`] to bytes.

use crate::ins::Instruction;
use crate::operand::Operand;
use crate::table::{Arg, Enc};

/// A byte buffer holding one encoded instruction.
///
/// Instructions are at most three bytes on the 8051, so this is a plain value
/// type with no allocation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Encoded {
    buf: [u8; 3],
    len: u8,
}

impl Encoded {
    /// The encoded bytes.
    pub fn as_slice(&self) -> &[u8] {
        &self.buf[..self.len as usize]
    }

    /// Number of encoded bytes.
    pub fn len(&self) -> usize {
        self.len as usize
    }

    /// `true` when nothing was encoded.
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// The bytes as a fixed array plus length.
    pub fn into_parts(self) -> ([u8; 3], u8) {
        (self.buf, self.len)
    }
}

impl core::ops::Deref for Encoded {
    type Target = [u8];
    fn deref(&self) -> &[u8] {
        self.as_slice()
    }
}

impl AsRef<[u8]> for Encoded {
    fn as_ref(&self) -> &[u8] {
        self.as_slice()
    }
}

impl PartialEq<[u8]> for Encoded {
    fn eq(&self, other: &[u8]) -> bool {
        self.as_slice() == other
    }
}

/// Why an instruction could not be encoded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EncodeError {
    /// The mnemonic/operand combination has no opcode.
    UndefinedInstruction,
    /// An `SJMP`/`JC`/`DJNZ`-style branch cannot reach its target: the
    /// displacement does not fit in a signed byte.
    RelativeOutOfRange {
        /// Address of the branch.
        from: u16,
        /// Requested destination.
        to: u16,
    },
    /// An `AJMP`/`ACALL` target is outside the 2 KiB page the instruction can
    /// address.
    PageOutOfRange {
        /// Address of the branch.
        from: u16,
        /// Requested destination.
        to: u16,
    },
}

impl core::fmt::Display for EncodeError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            EncodeError::UndefinedInstruction => {
                f.write_str("no 8051 opcode matches this instruction")
            }
            EncodeError::RelativeOutOfRange { from, to } => write!(
                f,
                "branch at {from:#06X} cannot reach {to:#06X}: displacement out of range \
                 (relative branches span -128..+127 bytes)"
            ),
            EncodeError::PageOutOfRange { from, to } => write!(
                f,
                "AJMP/ACALL at {from:#06X} cannot reach {to:#06X}: outside its 2 KiB page"
            ),
        }
    }
}

impl core::error::Error for EncodeError {}

/// Encode an instruction at [`Instruction::addr`].
///
/// Relative branch targets are supplied as [`Operand::Target`]; already-narrowed
/// [`Operand::Rel`], [`Operand::Addr11`] and [`Operand::Addr16`] values (as
/// produced by [`crate::decode`]) are accepted too, so decode/encode round-trips
/// are lossless.
pub fn encode(ins: &Instruction) -> Result<Encoded, EncodeError> {
    let def = ins.def().ok_or(EncodeError::UndefinedInstruction)?;
    let len = def.len();
    let mut buf = [0u8; 3];
    let mut opcode = def.base;
    let ops = ins.operands.as_slice();

    for (i, (arg, enc)) in def.args.iter().zip(def.enc.iter()).enumerate() {
        let op = ops[i];
        match *enc {
            Enc::Implicit => {}

            Enc::OpLow3 => opcode |= op.as_byte().unwrap_or(0) & 0x07,
            Enc::OpLow1 => opcode |= op.as_byte().unwrap_or(0) & 0x01,

            Enc::Addr11 => {
                let raw = match op {
                    Operand::Addr11(raw) => raw & 0x07FF,
                    Operand::Target(target) => {
                        let next = ins.addr.wrapping_add(len as u16);
                        if (target & 0xF800) != (next & 0xF800) {
                            return Err(EncodeError::PageOutOfRange {
                                from: ins.addr,
                                to: target,
                            });
                        }
                        target & 0x07FF
                    }
                    _ => return Err(EncodeError::UndefinedInstruction),
                };
                opcode |= ((raw >> 8) as u8 & 0x07) << 5;
                buf[1] = raw as u8;
            }

            Enc::Byte(off) => {
                let value = match (*arg, op) {
                    (Arg::Rel, Operand::Target(target)) => {
                        let next = ins.addr as i32 + len as i32;
                        let delta = target as i32 - next;
                        if !(-128..=127).contains(&delta) {
                            return Err(EncodeError::RelativeOutOfRange {
                                from: ins.addr,
                                to: target,
                            });
                        }
                        delta as i8 as u8
                    }
                    _ => op.as_byte().ok_or(EncodeError::UndefinedInstruction)?,
                };
                buf[off as usize] = value;
            }

            Enc::Word(off) => {
                let value = match op {
                    Operand::Target(t) | Operand::Addr16(t) => t,
                    other => other.as_word().ok_or(EncodeError::UndefinedInstruction)?,
                };
                buf[off as usize] = (value >> 8) as u8;
                buf[off as usize + 1] = value as u8;
            }
        }
    }

    buf[0] = opcode;
    Ok(Encoded { buf, len })
}
