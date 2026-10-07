//! The 8051 opcode map.
//!
//! The table below is the single source of truth for the whole toolchain: the
//! assembler encodes from it, the disassembler decodes from it, the simulator
//! prices instructions from it, and the size analyser reports with it.
//!
//! Each entry describes one *opcode family*. Families whose opcode has variable
//! bits (`INC Rn` is `0x08 | n`, `AJMP` is `0x01 | page << 5`) are stored once
//! with the variable bits zeroed in [`OpDef::base`]; [`OpDef::embed_mask`]
//! recovers the bit mask.

use crate::mnemonic::Mnemonic;

/// The shape of one operand position in an instruction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[allow(missing_docs)]
pub enum Arg {
    A,
    Ab,
    C,
    Dptr,
    Rn,
    AtRi,
    AtDptr,
    AtADptr,
    AtAPc,
    Imm8,
    Imm16,
    Direct,
    Bit,
    NotBit,
    Rel,
    Addr11,
    Addr16,
}

impl Arg {
    /// `true` when a `Target` operand may stand in for this argument.
    pub const fn accepts_target(self) -> bool {
        matches!(self, Arg::Rel | Arg::Addr11 | Arg::Addr16)
    }

    /// The fixed operand value for arguments that are encoded in the opcode.
    pub(crate) const fn implicit_operand(self) -> Option<crate::Operand> {
        use crate::Operand as O;
        Some(match self {
            Arg::A => O::A,
            Arg::Ab => O::Ab,
            Arg::C => O::C,
            Arg::Dptr => O::Dptr,
            Arg::AtDptr => O::AtDptr,
            Arg::AtADptr => O::AtADptr,
            Arg::AtAPc => O::AtAPc,
            _ => return None,
        })
    }
}

/// Where an operand's value lives inside the encoded instruction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[allow(missing_docs)]
pub enum Enc {
    /// The operand is implied by the opcode (`A`, `C`, `DPTR`, `@DPTR`, ...).
    Implicit,
    /// `Rn`: the register index occupies opcode bits 2..0 (`0x08 | n`).
    OpLow3,
    /// `@Ri`: the register index occupies opcode bit 0 (`0x06 | i`).
    OpLow1,
    /// `AJMP`/`ACALL`: page bits in opcode bits 7..5, low byte at offset 1.
    Addr11,
    /// One byte at the given instruction offset.
    Byte(u8),
    /// Two bytes, big-endian, at the given instruction offset.
    Word(u8),
}

const fn byte(n: u8) -> Enc {
    Enc::Byte(n)
}
const fn word(n: u8) -> Enc {
    Enc::Word(n)
}
const IMP: Enc = Enc::Implicit;
const L3: Enc = Enc::OpLow3;
const L1: Enc = Enc::OpLow1;
const A11: Enc = Enc::Addr11;

/// PSW flag side effects of an instruction.
pub mod flags {
    /// No PSW status bits are modified.
    pub const NONE: u8 = 0;
    /// Carry flag.
    pub const C: u8 = 1 << 0;
    /// Overflow flag.
    pub const OV: u8 = 1 << 1;
    /// Auxiliary carry flag.
    pub const AC: u8 = 1 << 2;
}

/// One opcode family.
#[derive(Debug, Clone, Copy)]
pub struct OpDef {
    /// The opcode with all variable (embedded) bits cleared.
    pub base: u8,
    /// The instruction mnemonic.
    pub mnemonic: Mnemonic,
    /// Operand shape, in source order.
    pub args: &'static [Arg],
    /// Encoding position for each entry of [`OpDef::args`].
    pub enc: &'static [Enc],
    /// Execution time in 8051 machine cycles (12 oscillator clocks each).
    pub cycles: u8,
    /// PSW flags written, as a mask of [`flags`].
    pub flags: u8,
}

impl OpDef {
    /// Bits of the opcode byte that vary between members of this family.
    pub const fn embed_mask(&self) -> u8 {
        let mut mask = 0u8;
        let mut i = 0;
        while i < self.enc.len() {
            mask |= match self.enc[i] {
                Enc::OpLow3 => 0x07,
                Enc::OpLow1 => 0x01,
                Enc::Addr11 => 0xE0,
                _ => 0x00,
            };
            i += 1;
        }
        mask
    }

    /// Encoded length in bytes.
    ///
    /// The length is the end of the furthest encoded operand, not the sum of
    /// the operand sizes: `MOV direct,#imm` stores two bytes at offsets 1 and 2
    /// and is three bytes long in total.
    pub const fn len(&self) -> u8 {
        let mut len = 1u8; // the opcode byte
        let mut i = 0;
        while i < self.enc.len() {
            let end = match self.enc[i] {
                Enc::Byte(off) => off + 1,
                Enc::Word(off) => off + 2,
                Enc::Addr11 => 2,
                Enc::Implicit | Enc::OpLow3 | Enc::OpLow1 => 1,
            };
            if end > len {
                len = end;
            }
            i += 1;
        }
        len
    }

    /// `true` if this family covers more than one opcode byte.
    pub const fn is_family(&self) -> bool {
        self.embed_mask() != 0
    }

    /// The opcode byte for the `n`th member of the family, in increasing order.
    pub const fn variant(&self, n: u8) -> u8 {
        match self.embed_mask() {
            0x07 => self.base | (n & 0x07),
            0x01 => self.base | (n & 0x01),
            0xE0 => self.base | ((n & 0x07) << 5),
            _ => self.base,
        }
    }

    /// Number of opcode bytes covered by this family.
    pub const fn variant_count(&self) -> u8 {
        match self.embed_mask() {
            0x07 => 8,
            0x01 => 2,
            0xE0 => 8,
            _ => 1,
        }
    }
}

macro_rules! op {
    ($base:literal, $mn:ident, $cycles:literal, $flags:expr, [ $($arg:ident => $enc:expr),* $(,)? ]) => {
        OpDef {
            base: $base,
            mnemonic: Mnemonic::$mn,
            args: &[$(Arg::$arg),*],
            enc: &[$( $enc ),*],
            cycles: $cycles,
            flags: $flags,
        }
    };
}

use self::flags::{AC as F_AC, C as F_C, NONE as F_NONE, OV as F_OV};

/// The complete 8051 opcode map, ordered by base opcode.
///
/// All 255 defined opcodes are covered; `0xA5` is the only undefined byte.
pub static OPCODES: &[OpDef] = &[
    op!(0x00, Nop, 1, F_NONE, []),
    op!(0x01, Ajmp, 2, F_NONE, [Addr11 => A11]),
    op!(0x02, Ljmp, 2, F_NONE, [Addr16 => word(1)]),
    op!(0x03, Rr, 1, F_NONE, [A => IMP]),
    op!(0x04, Inc, 1, F_NONE, [A => IMP]),
    op!(0x05, Inc, 1, F_NONE, [Direct => byte(1)]),
    op!(0x06, Inc, 1, F_NONE, [AtRi => L1]),
    op!(0x08, Inc, 1, F_NONE, [Rn => L3]),
    op!(0x10, Jbc, 2, F_NONE, [Bit => byte(1), Rel => byte(2)]),
    op!(0x11, Acall, 2, F_NONE, [Addr11 => A11]),
    op!(0x12, Lcall, 2, F_NONE, [Addr16 => word(1)]),
    op!(0x13, Rrc, 1, F_C, [A => IMP]),
    op!(0x14, Dec, 1, F_NONE, [A => IMP]),
    op!(0x15, Dec, 1, F_NONE, [Direct => byte(1)]),
    op!(0x16, Dec, 1, F_NONE, [AtRi => L1]),
    op!(0x18, Dec, 1, F_NONE, [Rn => L3]),
    op!(0x20, Jb, 2, F_NONE, [Bit => byte(1), Rel => byte(2)]),
    op!(0x22, Ret, 2, F_NONE, []),
    op!(0x23, Rl, 1, F_NONE, [A => IMP]),
    op!(0x24, Add, 1, F_C | F_OV | F_AC, [A => IMP, Imm8 => byte(1)]),
    op!(0x25, Add, 1, F_C | F_OV | F_AC, [A => IMP, Direct => byte(1)]),
    op!(0x26, Add, 1, F_C | F_OV | F_AC, [A => IMP, AtRi => L1]),
    op!(0x28, Add, 1, F_C | F_OV | F_AC, [A => IMP, Rn => L3]),
    op!(0x30, Jnb, 2, F_NONE, [Bit => byte(1), Rel => byte(2)]),
    op!(0x32, Reti, 2, F_NONE, []),
    op!(0x33, Rlc, 1, F_C, [A => IMP]),
    op!(0x34, Addc, 1, F_C | F_OV | F_AC, [A => IMP, Imm8 => byte(1)]),
    op!(0x35, Addc, 1, F_C | F_OV | F_AC, [A => IMP, Direct => byte(1)]),
    op!(0x36, Addc, 1, F_C | F_OV | F_AC, [A => IMP, AtRi => L1]),
    op!(0x38, Addc, 1, F_C | F_OV | F_AC, [A => IMP, Rn => L3]),
    op!(0x40, Jc, 2, F_NONE, [Rel => byte(1)]),
    op!(0x42, Orl, 1, F_NONE, [Direct => byte(1), A => IMP]),
    op!(0x43, Orl, 2, F_NONE, [Direct => byte(1), Imm8 => byte(2)]),
    op!(0x44, Orl, 1, F_NONE, [A => IMP, Imm8 => byte(1)]),
    op!(0x45, Orl, 1, F_NONE, [A => IMP, Direct => byte(1)]),
    op!(0x46, Orl, 1, F_NONE, [A => IMP, AtRi => L1]),
    op!(0x48, Orl, 1, F_NONE, [A => IMP, Rn => L3]),
    op!(0x50, Jnc, 2, F_NONE, [Rel => byte(1)]),
    op!(0x52, Anl, 1, F_NONE, [Direct => byte(1), A => IMP]),
    op!(0x53, Anl, 2, F_NONE, [Direct => byte(1), Imm8 => byte(2)]),
    op!(0x54, Anl, 1, F_NONE, [A => IMP, Imm8 => byte(1)]),
    op!(0x55, Anl, 1, F_NONE, [A => IMP, Direct => byte(1)]),
    op!(0x56, Anl, 1, F_NONE, [A => IMP, AtRi => L1]),
    op!(0x58, Anl, 1, F_NONE, [A => IMP, Rn => L3]),
    op!(0x60, Jz, 2, F_NONE, [Rel => byte(1)]),
    op!(0x62, Xrl, 1, F_NONE, [Direct => byte(1), A => IMP]),
    op!(0x63, Xrl, 2, F_NONE, [Direct => byte(1), Imm8 => byte(2)]),
    op!(0x64, Xrl, 1, F_NONE, [A => IMP, Imm8 => byte(1)]),
    op!(0x65, Xrl, 1, F_NONE, [A => IMP, Direct => byte(1)]),
    op!(0x66, Xrl, 1, F_NONE, [A => IMP, AtRi => L1]),
    op!(0x68, Xrl, 1, F_NONE, [A => IMP, Rn => L3]),
    op!(0x70, Jnz, 2, F_NONE, [Rel => byte(1)]),
    op!(0x72, Orl, 2, F_C, [C => IMP, Bit => byte(1)]),
    op!(0x73, Jmp, 2, F_NONE, [AtADptr => IMP]),
    op!(0x74, Mov, 1, F_NONE, [A => IMP, Imm8 => byte(1)]),
    op!(0x75, Mov, 2, F_NONE, [Direct => byte(1), Imm8 => byte(2)]),
    op!(0x76, Mov, 1, F_NONE, [AtRi => L1, Imm8 => byte(1)]),
    op!(0x78, Mov, 1, F_NONE, [Rn => L3, Imm8 => byte(1)]),
    op!(0x80, Sjmp, 2, F_NONE, [Rel => byte(1)]),
    op!(0x82, Anl, 2, F_C, [C => IMP, Bit => byte(1)]),
    op!(0x83, Movc, 2, F_NONE, [A => IMP, AtAPc => IMP]),
    op!(0x84, Div, 4, F_C | F_OV, [Ab => IMP]),
    // 0x85 is the only instruction whose object bytes are [src][dst].
    op!(0x85, Mov, 2, F_NONE, [Direct => byte(2), Direct => byte(1)]),
    op!(0x86, Mov, 2, F_NONE, [Direct => byte(1), AtRi => L1]),
    op!(0x88, Mov, 2, F_NONE, [Direct => byte(1), Rn => L3]),
    op!(0x90, Mov, 2, F_NONE, [Dptr => IMP, Imm16 => word(1)]),
    op!(0x92, Mov, 2, F_NONE, [Bit => byte(1), C => IMP]),
    op!(0x93, Movc, 2, F_NONE, [A => IMP, AtADptr => IMP]),
    op!(0x94, Subb, 1, F_C | F_OV | F_AC, [A => IMP, Imm8 => byte(1)]),
    op!(0x95, Subb, 1, F_C | F_OV | F_AC, [A => IMP, Direct => byte(1)]),
    op!(0x96, Subb, 1, F_C | F_OV | F_AC, [A => IMP, AtRi => L1]),
    op!(0x98, Subb, 1, F_C | F_OV | F_AC, [A => IMP, Rn => L3]),
    op!(0xA0, Orl, 2, F_C, [C => IMP, NotBit => byte(1)]),
    op!(0xA2, Mov, 1, F_C, [C => IMP, Bit => byte(1)]),
    op!(0xA3, Inc, 2, F_NONE, [Dptr => IMP]),
    op!(0xA4, Mul, 4, F_C | F_OV, [Ab => IMP]),
    op!(0xA6, Mov, 2, F_NONE, [AtRi => L1, Direct => byte(1)]),
    op!(0xA8, Mov, 2, F_NONE, [Rn => L3, Direct => byte(1)]),
    op!(0xB0, Anl, 2, F_C, [C => IMP, NotBit => byte(1)]),
    op!(0xB2, Cpl, 1, F_NONE, [Bit => byte(1)]),
    op!(0xB3, Cpl, 1, F_C, [C => IMP]),
    op!(0xB4, Cjne, 2, F_C, [A => IMP, Imm8 => byte(1), Rel => byte(2)]),
    op!(0xB5, Cjne, 2, F_C, [A => IMP, Direct => byte(1), Rel => byte(2)]),
    op!(0xB6, Cjne, 2, F_C, [AtRi => L1, Imm8 => byte(1), Rel => byte(2)]),
    op!(0xB8, Cjne, 2, F_C, [Rn => L3, Imm8 => byte(1), Rel => byte(2)]),
    op!(0xC0, Push, 2, F_NONE, [Direct => byte(1)]),
    op!(0xC2, Clr, 1, F_NONE, [Bit => byte(1)]),
    op!(0xC3, Clr, 1, F_C, [C => IMP]),
    op!(0xC4, Swap, 1, F_NONE, [A => IMP]),
    op!(0xC5, Xch, 1, F_NONE, [A => IMP, Direct => byte(1)]),
    op!(0xC6, Xch, 1, F_NONE, [A => IMP, AtRi => L1]),
    op!(0xC8, Xch, 1, F_NONE, [A => IMP, Rn => L3]),
    op!(0xD0, Pop, 2, F_NONE, [Direct => byte(1)]),
    op!(0xD2, Setb, 1, F_NONE, [Bit => byte(1)]),
    op!(0xD3, Setb, 1, F_C, [C => IMP]),
    op!(0xD4, Da, 1, F_C, [A => IMP]),
    op!(0xD5, Djnz, 2, F_NONE, [Direct => byte(1), Rel => byte(2)]),
    op!(0xD6, Xchd, 1, F_NONE, [A => IMP, AtRi => L1]),
    op!(0xD8, Djnz, 2, F_NONE, [Rn => L3, Rel => byte(1)]),
    op!(0xE0, Movx, 2, F_NONE, [A => IMP, AtDptr => IMP]),
    op!(0xE2, Movx, 2, F_NONE, [A => IMP, AtRi => L1]),
    op!(0xE4, Clr, 1, F_NONE, [A => IMP]),
    op!(0xE5, Mov, 1, F_NONE, [A => IMP, Direct => byte(1)]),
    op!(0xE6, Mov, 1, F_NONE, [A => IMP, AtRi => L1]),
    op!(0xE8, Mov, 1, F_NONE, [A => IMP, Rn => L3]),
    op!(0xF0, Movx, 2, F_NONE, [AtDptr => IMP, A => IMP]),
    op!(0xF2, Movx, 2, F_NONE, [AtRi => L1, A => IMP]),
    op!(0xF4, Cpl, 1, F_NONE, [A => IMP]),
    op!(0xF5, Mov, 1, F_NONE, [Direct => byte(1), A => IMP]),
    op!(0xF6, Mov, 1, F_NONE, [AtRi => L1, A => IMP]),
    op!(0xF8, Mov, 1, F_NONE, [Rn => L3, A => IMP]),
];

/// Opcode bytes with no defined instruction on the classic 8051.
pub const UNDEFINED_OPCODES: [u8; 1] = [0xA5];

/// Looks up [`OPCODES`] by opcode byte. `None` means the opcode is undefined.
pub static OPCODE_INDEX: [Option<u8>; 256] = build_index();

const fn build_index() -> [Option<u8>; 256] {
    let mut index = [None; 256];
    let mut i = 0usize;
    while i < OPCODES.len() {
        let def = &OPCODES[i];
        let mask = def.embed_mask();
        let mut n = 0u8;
        let count = def.variant_count();
        while n < count {
            index[def.variant(n) as usize] = Some(i as u8);
            n += 1;
        }
        let _ = mask;
        i += 1;
    }
    index
}

impl OpDef {
    /// Look up the definition that owns `opcode`.
    pub fn for_opcode(opcode: u8) -> Option<&'static OpDef> {
        OPCODE_INDEX[opcode as usize].map(|i| &OPCODES[i as usize])
    }

    /// Total number of opcode bytes covered by the table.
    pub fn covered_opcodes() -> usize {
        OPCODE_INDEX.iter().filter(|e| e.is_some()).count()
    }
}

/// The general class of an instruction, for analysis and disassembly.#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpcodeClass {
    /// Straight-line code.
    Data,
    /// Unconditional jump (`AJMP`, `LJMP`, `SJMP`, `JMP`, `RET`, `RETI`).
    Jump,
    /// Subroutine call (`ACALL`, `LCALL`).
    Call,
    /// Conditional branch carrying a `rel` operand.
    Branch,
}

impl OpDef {
    /// Classify this instruction.
    pub const fn class(&self) -> OpcodeClass {
        use Mnemonic as M;
        match self.mnemonic {
            M::Ajmp | M::Ljmp | M::Sjmp | M::Jmp => OpcodeClass::Jump,
            M::Ret | M::Reti => OpcodeClass::Jump,
            M::Acall | M::Lcall => OpcodeClass::Call,
            M::Jc | M::Jnc | M::Jz | M::Jnz | M::Jb | M::Jnb | M::Jbc | M::Cjne | M::Djnz => {
                OpcodeClass::Branch
            }
            _ => OpcodeClass::Data,
        }
    }
}

/// An operand value that does not fit the argument it is being matched against.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ValueRange {
    /// The argument slot the value was matched to.
    pub arg: Arg,
    /// The offending value.
    pub value: u32,
}

impl core::fmt::Display for ValueRange {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "value {:#X} does not fit {:?}", self.value, self.arg)
    }
}

impl Arg {
    /// Can an operand written in this source form fill this slot?
    pub const fn accepts(self, form: &crate::OperandForm) -> bool {
        use crate::OperandForm as F;
        match self {
            Arg::A => matches!(form, F::A),
            Arg::Ab => matches!(form, F::Ab),
            Arg::C => matches!(form, F::C),
            Arg::Dptr => matches!(form, F::Dptr),
            Arg::Rn => matches!(form, F::Rn(_)),
            Arg::AtRi => matches!(form, F::AtRi(_)),
            Arg::AtDptr => matches!(form, F::AtDptr),
            Arg::AtADptr => matches!(form, F::AtADptr),
            Arg::AtAPc => matches!(form, F::AtAPc),
            Arg::Imm8 | Arg::Imm16 => matches!(form, F::Imm(_)),
            Arg::NotBit => matches!(form, F::NotBit(_)),
            Arg::Direct | Arg::Bit | Arg::Rel | Arg::Addr11 | Arg::Addr16 => {
                matches!(form, F::Bare(_))
            }
        }
    }

    /// Can this already-typed operand fill this slot?
    ///
    /// `Direct` and `Bit` are interchangeable: `MOV C,0x30` and `MOV C,BITVAR`
    /// encode identically, and so do `MOV 0x30,C` and `MOV BITVAR,C`.
    pub const fn accepts_operand(self, op: crate::Operand) -> bool {
        use crate::Operand as O;
        match self {
            Arg::A => matches!(op, O::A),
            Arg::Ab => matches!(op, O::Ab),
            Arg::C => matches!(op, O::C),
            Arg::Dptr => matches!(op, O::Dptr),
            Arg::Rn => matches!(op, O::Rn(_)),
            Arg::AtRi => matches!(op, O::AtRi(_)),
            Arg::AtDptr => matches!(op, O::AtDptr),
            Arg::AtADptr => matches!(op, O::AtADptr),
            Arg::AtAPc => matches!(op, O::AtAPc),
            Arg::Imm8 => matches!(op, O::Imm8(_)),
            Arg::Imm16 => matches!(op, O::Imm16(_)),
            Arg::Direct => matches!(op, O::Direct(_) | O::Bit(_)),
            Arg::Bit => matches!(op, O::Bit(_) | O::Direct(_)),
            Arg::NotBit => matches!(op, O::NotBit(_)),
            Arg::Rel => matches!(op, O::Rel(_) | O::Target(_)),
            Arg::Addr11 => matches!(op, O::Addr11(_) | O::Target(_)),
            Arg::Addr16 => matches!(op, O::Addr16(_) | O::Target(_)),
        }
    }

    /// Convert a source form into the concrete operand this slot expects.
    pub const fn materialize(
        self,
        form: &crate::OperandForm,
    ) -> Result<crate::Operand, ValueRange> {
        use crate::Operand as O;
        use crate::OperandForm as F;
        macro_rules! range_err {
            ($value:expr) => {
                Err(ValueRange {
                    arg: self,
                    value: $value,
                })
            };
        }
        match (self, form) {
            (Arg::A, _) => Ok(O::A),
            (Arg::Ab, _) => Ok(O::Ab),
            (Arg::C, _) => Ok(O::C),
            (Arg::Dptr, _) => Ok(O::Dptr),
            (Arg::AtDptr, _) => Ok(O::AtDptr),
            (Arg::AtADptr, _) => Ok(O::AtADptr),
            (Arg::AtAPc, _) => Ok(O::AtAPc),
            (Arg::Rn, F::Rn(n)) => Ok(O::Rn(*n)),
            (Arg::AtRi, F::AtRi(n)) => Ok(O::AtRi(*n)),
            (Arg::Imm8, F::Imm(v)) => {
                if *v <= 0xFF {
                    Ok(O::Imm8(*v as u8))
                } else {
                    range_err!(*v)
                }
            }
            (Arg::Imm16, F::Imm(v)) => {
                if *v <= 0xFFFF {
                    Ok(O::Imm16(*v as u16))
                } else {
                    range_err!(*v)
                }
            }
            (Arg::Direct, F::Bare(v)) => {
                if *v <= 0xFF {
                    Ok(O::Direct(*v as u8))
                } else {
                    range_err!(*v)
                }
            }
            (Arg::Bit, F::Bare(v)) => {
                if *v <= 0xFF {
                    Ok(O::Bit(*v as u8))
                } else {
                    range_err!(*v)
                }
            }
            (Arg::NotBit, F::NotBit(v)) => {
                if *v <= 0xFF {
                    Ok(O::NotBit(*v as u8))
                } else {
                    range_err!(*v)
                }
            }
            (Arg::Rel | Arg::Addr11 | Arg::Addr16, F::Bare(v)) => {
                if *v <= 0xFFFF {
                    Ok(O::Target(*v as u16))
                } else {
                    range_err!(*v)
                }
            }
            (_, other) => match other.value() {
                Some(value) => range_err!(value),
                None => range_err!(0),
            },
        }
    }
}
