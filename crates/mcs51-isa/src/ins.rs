//! The [`Instruction`] type: a mnemonic plus typed operands, with shape lookup.

use core::fmt;

use crate::mnemonic::Mnemonic;
use crate::operand::{Operand, OperandForm, Operands};
use crate::sfr;
use crate::table::{OPCODES, OpDef, OpcodeClass, ValueRange};

/// A decoded or hand-built 8051 instruction.
///
/// `addr` is the code address the instruction will be placed at. It is needed
/// because 8051 branches are relative: `SJMP`, `JC`, `DJNZ` and friends store a
/// signed displacement from the *end* of the instruction, and `AJMP`/`ACALL`
/// store an 11-bit address within the 2 KiB page containing the following
/// instruction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Instruction {
    /// Code address this instruction is located at.
    pub addr: u16,
    /// The operation.
    pub mnemonic: Mnemonic,
    /// Operands, in source order.
    pub operands: Operands,
}

/// Why an instruction could not be matched to an opcode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LookupError {
    /// No opcode with this mnemonic and operand shape exists.
    NoSuchInstruction {
        /// The mnemonic that was looked up.
        mnemonic: Mnemonic,
        /// How many operands were supplied.
        arity: u8,
    },
    /// More than one opcode matched, which the operand shape should prevent.
    Ambiguous {
        /// The mnemonic that was looked up.
        mnemonic: Mnemonic,
        /// How many operands were supplied.
        arity: u8,
    },
    /// An operand value was outside the range the shape allows.
    Range(ValueRange),
}

impl fmt::Display for LookupError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LookupError::NoSuchInstruction { mnemonic, arity } => {
                write!(f, "{mnemonic} has no {arity}-operand form on the 8051")
            }
            LookupError::Ambiguous { mnemonic, arity } => {
                write!(
                    f,
                    "{mnemonic} with {arity} operands is ambiguous on the 8051"
                )
            }
            LookupError::Range(r) => write!(f, "{r}"),
        }
    }
}

impl core::error::Error for LookupError {}

impl Instruction {
    /// Build an instruction from already-typed operands.
    pub fn new(addr: u16, mnemonic: Mnemonic, operands: &[Operand]) -> Self {
        Self {
            addr,
            mnemonic,
            operands: Operands::from_slice(operands),
        }
    }

    /// Build an instruction from source-level operand forms, resolving the
    /// ambiguous ones (`0x30` is a direct, a bit or a jump target depending on
    /// the instruction).
    pub fn select(
        addr: u16,
        mnemonic: Mnemonic,
        forms: &[OperandForm],
    ) -> Result<(Self, &'static OpDef), LookupError> {
        let mut found: Option<(&'static OpDef, Operands)> = None;
        for def in OPCODES.iter().filter(|d| d.mnemonic == mnemonic) {
            if def.args.len() != forms.len() {
                continue;
            }
            if !def
                .args
                .iter()
                .zip(forms)
                .all(|(arg, form)| arg.accepts(form))
            {
                continue;
            }
            let mut ops = Operands::new();
            let mut matched = true;
            for (arg, form) in def.args.iter().zip(forms) {
                match arg.materialize(form) {
                    Ok(op) => ops.push(op),
                    Err(range) => {
                        if range.value > 0xFFFF {
                            return Err(LookupError::Range(range));
                        }
                        matched = false;
                        break;
                    }
                }
            }
            if !matched {
                continue;
            }
            if found.is_some() {
                return Err(LookupError::Ambiguous {
                    mnemonic,
                    arity: forms.len() as u8,
                });
            }
            found = Some((def, ops));
        }
        match found {
            Some((def, operands)) => Ok((
                Self {
                    addr,
                    mnemonic,
                    operands,
                },
                def,
            )),
            None => Err(LookupError::NoSuchInstruction {
                mnemonic,
                arity: forms.len() as u8,
            }),
        }
    }

    /// The opcode definition this instruction encodes to, if it exists.
    pub fn def(&self) -> Option<&'static OpDef> {
        OPCODES.iter().find(|d| {
            d.mnemonic == self.mnemonic
                && d.args.len() == self.operands.len()
                && d.args
                    .iter()
                    .zip(self.operands.as_slice())
                    .all(|(arg, op)| arg.accepts_operand(*op))
        })
    }

    /// Instruction length in bytes, or 1 if the opcode is undefined.
    pub fn len(&self) -> u8 {
        self.def().map_or(1, |d| d.len())
    }

    /// `true` when this instruction matches a defined opcode.
    pub fn is_defined(&self) -> bool {
        self.def().is_some()
    }

    /// Machine cycles, or 0 if the opcode is undefined.
    pub fn cycles(&self) -> u8 {
        self.def().map_or(0, |d| d.cycles)
    }

    /// PSW flag side effects, or 0 if the opcode is undefined.
    pub fn flags(&self) -> u8 {
        self.def().map_or(0, |d| d.flags)
    }

    /// Instruction class, for control-flow analysis.
    pub fn class(&self) -> OpcodeClass {
        self.def().map_or(OpcodeClass::Data, |d| d.class())
    }

    /// The absolute code address this instruction transfers to, if any.
    ///
    /// For `AJMP`/`ACALL` this applies the hardware rule: the target lies in the
    /// 2 KiB page containing the instruction *after* this one.
    pub fn target(&self) -> Option<u16> {
        let next = self.addr.wrapping_add(self.len() as u16);
        for op in self.operands.iter() {
            match *op {
                Operand::Rel(r) => return Some(next.wrapping_add(r as i16 as u16)),
                Operand::Addr11(raw) => return Some((next & 0xF800) | (raw & 0x07FF)),
                Operand::Addr16(t) | Operand::Target(t) => return Some(t),
                _ => {}
            }
        }
        None
    }

    fn fmt_operand(&self, f: &mut fmt::Formatter<'_>, op: Operand) -> fmt::Result {
        match op {
            Operand::A => f.write_str("A"),
            Operand::Ab => f.write_str("AB"),
            Operand::C => f.write_str("C"),
            Operand::Dptr => f.write_str("DPTR"),
            Operand::AtDptr => f.write_str("@DPTR"),
            Operand::AtADptr => f.write_str("@A+DPTR"),
            Operand::AtAPc => f.write_str("@A+PC"),
            Operand::Rn(n) => write!(f, "R{n}"),
            Operand::AtRi(i) => write!(f, "@R{i}"),
            Operand::Imm8(v) => write!(f, "#0x{v:02X}"),
            Operand::Imm16(v) => write!(f, "#0x{v:04X}"),
            Operand::Direct(d) => write!(f, "{}", sfr::format_direct(d)),
            Operand::Bit(b) => write!(f, "{}", sfr::format_bit(b)),
            Operand::NotBit(b) => {
                f.write_str("/")?;
                write!(f, "{}", sfr::format_bit(b))
            }
            Operand::Rel(_) | Operand::Addr11(_) => {
                write!(f, "0x{:04X}", self.target().unwrap_or(0))
            }
            Operand::Addr16(t) | Operand::Target(t) => write!(f, "0x{t:04X}"),
        }
    }
}

impl fmt::Display for Instruction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.mnemonic.as_str())?;
        for (i, op) in self.operands.iter().enumerate() {
            f.write_str(if i == 0 { " " } else { "," })?;
            self.fmt_operand(f, *op)?;
        }
        Ok(())
    }
}
