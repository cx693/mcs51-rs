//! # mcs51-isa
//!
//! The Intel MCS-51 (8051) instruction set, described once and used everywhere
//! in the `mcs51-rs` toolchain.
//!
//! This crate is `no_std` and allocation free: the whole instruction set is a
//! `static` table, and decoding/encoding work on fixed-size stack buffers. The
//! same code that the host toolchain uses to *assemble* firmware can therefore
//! also run on a target that has to *disassemble* or *patch* code at runtime.
//!
//! ## What is here
//!
//! * [`OpDef`] / [`OPCODES`] — the complete 8051 opcode map: 255 defined opcodes,
//!   with operand shapes, encoding positions, machine-cycle cost and PSW flags.
//! * [`encode`] / [`decode`] — byte-level assembler and disassembler.
//! * [`sfr`] — special function register and bit addresses.
//! * [`mem`] — the Harvard memory map (CODE/DATA/IDATA/XDATA/BIT/SFR).
//!
//! ## Example
//!
//! ```
//! use mcs51_isa::{encode, decode, Instruction, Mnemonic, Operand};
//!
//! // MOV A, #0x42
//! let ins = Instruction::new(0x0000, Mnemonic::Mov, &[Operand::A, Operand::Imm8(0x42)]);
//! let bytes = encode(&ins).unwrap();
//! assert_eq!(&bytes.as_slice(), &[0x74, 0x42]);
//!
//! // ... and back again.
//! let back = decode(0x0000, bytes.as_slice()).unwrap();
//! assert_eq!(back.mnemonic, Mnemonic::Mov);
//! assert_eq!(back.operands.as_slice(), &[Operand::A, Operand::Imm8(0x42)]);
//! ```

#![no_std]
#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod mem;
pub mod sfr;

mod decode;
mod encode;
mod ins;
mod mnemonic;
mod operand;
mod table;

pub use decode::{
    DecodeError, decode, decode_at, instruction_length, is_defined_opcode, opcode_def,
    opcode_mnemonic,
};
pub use encode::{EncodeError, Encoded, encode};
pub use ins::{Instruction, LookupError};
pub use mnemonic::Mnemonic;
pub use operand::{Operand, OperandForm, OperandKind, Operands};
pub use table::{
    Arg, Enc, OPCODE_INDEX, OPCODES, OpDef, OpcodeClass, UNDEFINED_OPCODES, ValueRange,
};

/// PSW flag side-effect masks, as recorded in [`OpDef::flags`].
pub use table::flags;

/// The number of oscillator clocks in one 8051 machine cycle.
pub const CLOCKS_PER_CYCLE: u32 = 12;

/// The largest instruction length in bytes.
pub const MAX_INSTRUCTION_LEN: usize = 3;

/// The reset vector: where the 8051 starts executing after reset.
pub const RESET_VECTOR: u16 = 0x0000;

/// Interrupt vector addresses in ascending priority order.
pub const INTERRUPT_VECTORS: [u16; 8] = [
    0x0003, // INT0   - external interrupt 0
    0x000B, // Timer0 - timer 0 overflow
    0x0013, // INT1   - external interrupt 1
    0x001B, // Timer1 - timer 1 overflow
    0x0023, // Serial - UART receive/transmit
    0x002B, // Timer2 - timer 2 (8052)
    0x0033, // reserved on the 8051
    0x003B, // reserved on the 8051
];

/// Names of the interrupt vectors, indexed like [`INTERRUPT_VECTORS`].
pub const INTERRUPT_NAMES: [&str; 8] = [
    "INT0",
    "TIMER0",
    "INT1",
    "TIMER1",
    "SERIAL",
    "TIMER2",
    "RESERVED6",
    "RESERVED7",
];
