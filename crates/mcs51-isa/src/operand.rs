//! Instruction operands.

/// A resolved operand value.
///
/// Operands carry *values*, not textual forms. The assembler produces
/// [`Operand::Target`] for anything that names a code address and lets
/// [`crate::encode`] pick the right 8-bit relative / 11-bit page / 16-bit
/// absolute representation from the matched [`crate::OpDef`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Operand {
    /// The accumulator, `A`.
    A,
    /// The `AB` register pair used by `MUL`/`DIV`.
    Ab,
    /// The carry flag, `C`.
    C,
    /// The 16-bit data pointer, `DPTR`.
    Dptr,
    /// `@DPTR` — external data through the data pointer.
    AtDptr,
    /// `@A+DPTR` — table lookup through code memory.
    AtADptr,
    /// `@A+PC` — table lookup relative to the program counter.
    AtAPc,
    /// `R0`..`R7`, the active register bank.
    Rn(u8),
    /// `@R0` / `@R1` — indirect internal data.
    AtRi(u8),
    /// An 8-bit immediate, `#nn`.
    Imm8(u8),
    /// A 16-bit immediate, `#nnnn`.
    Imm16(u16),
    /// A direct internal data / SFR address.
    Direct(u8),
    /// A bit address.
    Bit(u8),
    /// A complemented bit address, `/bit`.
    NotBit(u8),
    /// An absolute code address not yet narrowed to rel/addr11/addr16.
    Target(u16),
    /// An 8-bit signed branch displacement (produced by decoding).
    Rel(i8),
    /// An 11-bit page-relative address (produced by decoding `AJMP`/`ACALL`).
    Addr11(u16),
    /// A 16-bit absolute address (produced by decoding `LJMP`/`LCALL`).
    Addr16(u16),
}

impl Default for Operand {
    fn default() -> Self {
        Operand::A
    }
}

/// The *class* of an operand, ignoring its value.
///
/// This is what [`crate::Arg`] matching works on and what error messages quote.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[allow(missing_docs)]
pub enum OperandKind {
    A,
    Ab,
    C,
    Dptr,
    AtDptr,
    AtADptr,
    AtAPc,
    Rn,
    AtRi,
    Imm8,
    Imm16,
    Direct,
    Bit,
    NotBit,
    Target,
    Rel,
    Addr11,
    Addr16,
}

impl core::fmt::Display for OperandKind {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(match self {
            OperandKind::A => "A",
            OperandKind::Ab => "AB",
            OperandKind::C => "C",
            OperandKind::Dptr => "DPTR",
            OperandKind::AtDptr => "@DPTR",
            OperandKind::AtADptr => "@A+DPTR",
            OperandKind::AtAPc => "@A+PC",
            OperandKind::Rn => "Rn",
            OperandKind::AtRi => "@Ri",
            OperandKind::Imm8 => "#imm8",
            OperandKind::Imm16 => "#imm16",
            OperandKind::Direct => "direct",
            OperandKind::Bit => "bit",
            OperandKind::NotBit => "/bit",
            OperandKind::Target => "target",
            OperandKind::Rel => "rel",
            OperandKind::Addr11 => "addr11",
            OperandKind::Addr16 => "addr16",
        })
    }
}

impl Operand {
    /// The operand class, ignoring the value.
    pub const fn kind(self) -> OperandKind {
        match self {
            Operand::A => OperandKind::A,
            Operand::Ab => OperandKind::Ab,
            Operand::C => OperandKind::C,
            Operand::Dptr => OperandKind::Dptr,
            Operand::AtDptr => OperandKind::AtDptr,
            Operand::AtADptr => OperandKind::AtADptr,
            Operand::AtAPc => OperandKind::AtAPc,
            Operand::Rn(_) => OperandKind::Rn,
            Operand::AtRi(_) => OperandKind::AtRi,
            Operand::Imm8(_) => OperandKind::Imm8,
            Operand::Imm16(_) => OperandKind::Imm16,
            Operand::Direct(_) => OperandKind::Direct,
            Operand::Bit(_) => OperandKind::Bit,
            Operand::NotBit(_) => OperandKind::NotBit,
            Operand::Target(_) => OperandKind::Target,
            Operand::Rel(_) => OperandKind::Rel,
            Operand::Addr11(_) => OperandKind::Addr11,
            Operand::Addr16(_) => OperandKind::Addr16,
        }
    }

    /// The single byte this operand encodes to, if it is byte sized.
    pub const fn as_byte(self) -> Option<u8> {
        match self {
            Operand::Rn(n) | Operand::AtRi(n) => Some(n),
            Operand::Imm8(v) | Operand::Direct(v) | Operand::Bit(v) | Operand::NotBit(v) => Some(v),
            Operand::Rel(v) => Some(v as u8),
            _ => None,
        }
    }

    /// The 16-bit value this operand encodes to, if it is word sized.
    pub const fn as_word(self) -> Option<u16> {
        match self {
            Operand::Imm16(v) | Operand::Addr11(v) | Operand::Addr16(v) | Operand::Target(v) => {
                Some(v)
            }
            _ => None,
        }
    }
}

/// An operand as it appears in source text, before the instruction shape is
/// known.
///
/// `0x30` is a direct address in `INC 0x30`, a bit address in `CLR 0x30` and a
/// jump destination in `SJMP 0x30`. Only the matched [`crate::OpDef`] can say
/// which, so the parser stops here and [`crate::Instruction::select`] finishes
/// the job.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OperandForm {
    /// `A`
    A,
    /// `AB`
    Ab,
    /// `C`
    C,
    /// `DPTR`
    Dptr,
    /// `R0`..`R7`
    Rn(u8),
    /// `@R0` / `@R1`
    AtRi(u8),
    /// `@DPTR`
    AtDptr,
    /// `@A+DPTR`
    AtADptr,
    /// `@A+PC`
    AtAPc,
    /// `#value`
    Imm(u32),
    /// `/value`
    NotBit(u32),
    /// A bare expression: direct address, bit address or code address.
    Bare(u32),
}

impl OperandForm {
    /// The numeric value carried by this form, if any.
    pub const fn value(self) -> Option<u32> {
        match self {
            OperandForm::Rn(v) | OperandForm::AtRi(v) => Some(v as u32),
            OperandForm::Imm(v) | OperandForm::NotBit(v) | OperandForm::Bare(v) => Some(v),
            _ => None,
        }
    }

    /// The textual shape, for diagnostics.
    pub const fn describe(self) -> &'static str {
        match self {
            OperandForm::A => "A",
            OperandForm::Ab => "AB",
            OperandForm::C => "C",
            OperandForm::Dptr => "DPTR",
            OperandForm::Rn(_) => "Rn",
            OperandForm::AtRi(_) => "@Ri",
            OperandForm::AtDptr => "@DPTR",
            OperandForm::AtADptr => "@A+DPTR",
            OperandForm::AtAPc => "@A+PC",
            OperandForm::Imm(_) => "#imm",
            OperandForm::NotBit(_) => "/bit",
            OperandForm::Bare(_) => "expression",
        }
    }
}

/// A fixed-capacity operand list (at most [`crate::MAX_INSTRUCTION_LEN`] items,
/// which is the maximum operand count of any 8051 instruction).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Operands {
    buf: [Operand; 3],
    len: u8,
}

impl Operands {
    /// An empty operand list.
    pub const fn new() -> Self {
        Self {
            buf: [Operand::A; 3],
            len: 0,
        }
    }

    /// Build an operand list from a slice, truncating past three operands.
    pub fn from_slice(items: &[Operand]) -> Self {
        let mut out = Self::new();
        for item in items.iter().take(3) {
            out.push(*item);
        }
        out
    }

    /// Append one operand.
    pub fn push(&mut self, op: Operand) {
        if (self.len as usize) < self.buf.len() {
            self.buf[self.len as usize] = op;
            self.len += 1;
        }
    }

    /// Replace one operand in place.
    ///
    /// Out-of-range indices are ignored.
    pub fn set(&mut self, index: usize, op: Operand) {
        if index < self.len as usize {
            self.buf[index] = op;
        }
    }

    /// The operands as a slice.
    pub fn as_slice(&self) -> &[Operand] {
        &self.buf[..self.len as usize]
    }

    /// Number of operands.
    pub fn len(&self) -> usize {
        self.len as usize
    }

    /// `true` if there are no operands.
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Iterate the operands.
    pub fn iter(&self) -> core::slice::Iter<'_, Operand> {
        self.as_slice().iter()
    }
}

impl core::ops::Deref for Operands {
    type Target = [Operand];
    fn deref(&self) -> &[Operand] {
        self.as_slice()
    }
}

impl<'a> IntoIterator for &'a Operands {
    type Item = &'a Operand;
    type IntoIter = core::slice::Iter<'a, Operand>;
    fn into_iter(self) -> Self::IntoIter {
        self.as_slice().iter()
    }
}
