//! Instruction decoding: bytes to [`Instruction`].

use crate::ins::Instruction;
use crate::mnemonic::Mnemonic;
use crate::operand::{Operand, Operands};
use crate::table::{Enc, OPCODE_INDEX, OPCODES, OpDef};

/// Why a byte sequence could not be decoded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecodeError {
    /// The opcode byte has no defined instruction (only `0xA5` on the 8051).
    UndefinedOpcode(u8),
    /// The buffer ended before the instruction did.
    Truncated {
        /// Bytes the instruction needs.
        needed: u8,
        /// Bytes that were available.
        available: usize,
    },
}

impl core::fmt::Display for DecodeError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            DecodeError::UndefinedOpcode(op) => write!(f, "undefined 8051 opcode {op:#04X}"),
            DecodeError::Truncated { needed, available } => write!(
                f,
                "truncated instruction: needs {needed} bytes, only {available} available"
            ),
        }
    }
}

impl core::error::Error for DecodeError {}

/// The encoded length in bytes of the instruction starting with `opcode`.
///
/// Undefined opcodes report a length of 1 so that disassembly can resynchronize.
pub fn instruction_length(opcode: u8) -> u8 {
    match OPCODE_INDEX[opcode as usize] {
        Some(idx) => OPCODES[idx as usize].len(),
        None => 1,
    }
}

/// Look up the opcode family for a byte, without decoding operands.
pub fn opcode_def(opcode: u8) -> Option<&'static OpDef> {
    OPCODE_INDEX[opcode as usize].map(|i| &OPCODES[i as usize])
}

/// Decode one instruction that starts at `addr`.
///
/// `bytes` must start with the opcode; extra bytes are ignored.
pub fn decode(addr: u16, bytes: &[u8]) -> Result<Instruction, DecodeError> {
    let &opcode = bytes.first().ok_or(DecodeError::Truncated {
        needed: 1,
        available: 0,
    })?;
    let def = opcode_def(opcode).ok_or(DecodeError::UndefinedOpcode(opcode))?;
    let len = def.len();
    if bytes.len() < len as usize {
        return Err(DecodeError::Truncated {
            needed: len,
            available: bytes.len(),
        });
    }

    let mut operands = Operands::new();
    for (arg, enc) in def.args.iter().zip(def.enc.iter()) {
        let op = match *enc {
            Enc::Implicit => arg
                .implicit_operand()
                .expect("implicit encoding requires an implied argument"),
            Enc::OpLow3 => Operand::Rn(opcode & 0x07),
            Enc::OpLow1 => Operand::AtRi(opcode & 0x01),
            Enc::Addr11 => Operand::Addr11((((opcode >> 5) as u16) & 0x07) << 8 | bytes[1] as u16),
            Enc::Byte(off) => match arg {
                crate::table::Arg::Rel => Operand::Rel(bytes[off as usize] as i8),
                crate::table::Arg::Direct => Operand::Direct(bytes[off as usize]),
                crate::table::Arg::Bit => Operand::Bit(bytes[off as usize]),
                crate::table::Arg::NotBit => Operand::NotBit(bytes[off as usize]),
                crate::table::Arg::Imm8 => Operand::Imm8(bytes[off as usize]),
                other => unreachable_encoding(*other),
            },
            Enc::Word(off) => {
                let value = ((bytes[off as usize] as u16) << 8) | bytes[off as usize + 1] as u16;
                match arg {
                    crate::table::Arg::Addr16 => Operand::Addr16(value),
                    crate::table::Arg::Imm16 => Operand::Imm16(value),
                    other => unreachable_encoding(*other),
                }
            }
        };
        operands.push(op);
    }

    Ok(Instruction {
        addr,
        mnemonic: def.mnemonic,
        operands,
    })
}

#[cold]
fn unreachable_encoding(arg: crate::table::Arg) -> ! {
    panic!("opcode table has a byte/word encoding for {arg:?}");
}

/// Decode the instruction at `addr`, reading from a code-memory image.
pub fn decode_at(image: &[u8], addr: u16) -> Result<Instruction, DecodeError> {
    let off = addr as usize;
    let opcode = *image.get(off).ok_or(DecodeError::Truncated {
        needed: 1,
        available: 0,
    })?;
    let len = instruction_length(opcode) as usize;
    if image.len() < off + len {
        return Err(DecodeError::Truncated {
            needed: len as u8,
            available: image.len().saturating_sub(off),
        });
    }
    decode(addr, &image[off..off + len])
}

/// `true` if the byte is a defined 8051 opcode.
pub fn is_defined_opcode(opcode: u8) -> bool {
    OPCODE_INDEX[opcode as usize].is_some()
}

/// The mnemonic for an opcode byte, if defined.
pub fn opcode_mnemonic(opcode: u8) -> Option<Mnemonic> {
    opcode_def(opcode).map(|d| d.mnemonic)
}
