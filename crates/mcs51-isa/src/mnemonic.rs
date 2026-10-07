//! 8051 mnemonics.

/// An 8051 instruction mnemonic.
///
/// Mnemonics are deliberately *not* unique per opcode: `MOV` alone covers 29
/// distinct opcodes, and `INC` covers six. They are disambiguated by operand
/// shape — see [`crate::OpDef`] and [`crate::Arg`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[allow(missing_docs)]
pub enum Mnemonic {
    /// Unrecognized opcode byte (only `0xA5` on the classic 8051).
    Unknown,
    Nop,
    Ajmp,
    Lcall,
    Ljmp,
    Acall,
    Rr,
    Inc,
    Dec,
    Jbc,
    Rrc,
    Rl,
    Rlc,
    Add,
    Addc,
    Subb,
    Orl,
    Anl,
    Xrl,
    Jc,
    Jnc,
    Jz,
    Jnz,
    Jb,
    Jnb,
    Ret,
    Reti,
    Mul,
    Div,
    Da,
    Mov,
    Movc,
    Movx,
    Cjne,
    Push,
    Pop,
    Xch,
    Xchd,
    Swap,
    Clr,
    Setb,
    Cpl,
    Jmp,
    Sjmp,
    Djnz,
}

macro_rules! mnemonics {
    ($($variant:ident => $text:literal),* $(,)?) => {
        impl Mnemonic {
            /// The canonical upper-case assembly spelling.
            pub const fn as_str(self) -> &'static str {
                match self {
                    Mnemonic::Unknown => "???",
                    $(Mnemonic::$variant => $text),*
                }
            }

            /// Every mnemonic that has at least one defined opcode.
            pub const ALL: &'static [Mnemonic] = &[$(Mnemonic::$variant),*];
        }
    };
}

mnemonics! {
    Nop => "NOP",
    Ajmp => "AJMP",
    Lcall => "LCALL",
    Ljmp => "LJMP",
    Acall => "ACALL",
    Rr => "RR",
    Inc => "INC",
    Dec => "DEC",
    Jbc => "JBC",
    Rrc => "RRC",
    Rl => "RL",
    Rlc => "RLC",
    Add => "ADD",
    Addc => "ADDC",
    Subb => "SUBB",
    Orl => "ORL",
    Anl => "ANL",
    Xrl => "XRL",
    Jc => "JC",
    Jnc => "JNC",
    Jz => "JZ",
    Jnz => "JNZ",
    Jb => "JB",
    Jnb => "JNB",
    Ret => "RET",
    Reti => "RETI",
    Mul => "MUL",
    Div => "DIV",
    Da => "DA",
    Mov => "MOV",
    Movc => "MOVC",
    Movx => "MOVX",
    Cjne => "CJNE",
    Push => "PUSH",
    Pop => "POP",
    Xch => "XCH",
    Xchd => "XCHD",
    Swap => "SWAP",
    Clr => "CLR",
    Setb => "SETB",
    Cpl => "CPL",
    Jmp => "JMP",
    Sjmp => "SJMP",
    Djnz => "DJNZ",
}

impl Mnemonic {
    /// Parse a mnemonic in any letter case.
    pub fn parse(s: &str) -> Option<Mnemonic> {
        if s.eq_ignore_ascii_case("???") {
            return Some(Mnemonic::Unknown);
        }
        Mnemonic::ALL
            .iter()
            .copied()
            .find(|m| m.as_str().eq_ignore_ascii_case(s))
    }

    /// `true` if this mnemonic always transfers control elsewhere.
    pub const fn is_unconditional_jump(self) -> bool {
        matches!(
            self,
            Mnemonic::Ajmp
                | Mnemonic::Ljmp
                | Mnemonic::Sjmp
                | Mnemonic::Jmp
                | Mnemonic::Ret
                | Mnemonic::Reti
        )
    }

    /// `true` if this mnemonic reads the `rel` operand (a conditional branch).
    pub const fn is_conditional_branch(self) -> bool {
        matches!(
            self,
            Mnemonic::Jc
                | Mnemonic::Jnc
                | Mnemonic::Jz
                | Mnemonic::Jnz
                | Mnemonic::Jb
                | Mnemonic::Jnb
                | Mnemonic::Jbc
                | Mnemonic::Cjne
                | Mnemonic::Djnz
        )
    }
}

impl core::fmt::Display for Mnemonic {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(self.as_str())
    }
}
