//! The 8051 memory map.
//!
//! The 8051 is a Harvard architecture: program memory (CODE) and data memory
//! are addressed by different instructions. Data memory itself is split into
//! overlapping spaces that the *instruction*, not the address, selects.

/// A distinct address space of the 8051.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[allow(missing_docs)]
pub enum Space {
    /// Program memory, read with `MOVC` / executed from. Up to 64 KiB.
    Code,
    /// The lower 128 bytes of internal RAM: register banks, bit area, scratch.
    Data,
    /// All 256 bytes of internal RAM, reachable through `@Ri`.
    Idata,
    /// Off-chip data memory, reachable through `MOVX`. Up to 64 KiB.
    Xdata,
    /// The bit-addressable area: RAM `0x20..=0x2F` and every SFR whose address
    /// is a multiple of 8.
    Bit,
    /// The 128 special function registers, `0x80..=0xFF`.
    Sfr,
}

impl Space {
    /// Human-readable name.
    pub const fn name(self) -> &'static str {
        match self {
            Space::Code => "CODE",
            Space::Data => "DATA",
            Space::Idata => "IDATA",
            Space::Xdata => "XDATA",
            Space::Bit => "BIT",
            Space::Sfr => "SFR",
        }
    }
}

/// Size of internal RAM.
pub const IRAM_SIZE: usize = 256;
/// Size of the directly addressable internal RAM.
pub const DATA_SIZE: usize = 128;
/// Size of the bit-addressable RAM area in bytes.
pub const BIT_RAM_BYTES: u8 = 16;
/// First byte of the bit-addressable RAM area.
pub const BIT_RAM_BASE: u8 = 0x20;
/// Size of the addressable code space.
pub const CODE_SIZE: usize = 65536;
/// Size of the addressable external data space.
pub const XDATA_SIZE: usize = 65536;

/// Which space a *direct* address refers to.
pub const fn classify_direct(addr: u8) -> Space {
    if addr < 0x80 { Space::Data } else { Space::Sfr }
}

/// The byte of internal RAM that holds a RAM bit address.
pub const fn bit_byte(addr: u8) -> u8 {
    if addr < 0x80 {
        BIT_RAM_BASE + (addr >> 3)
    } else {
        addr & 0xF8
    }
}

/// The bit mask (within [`bit_byte`]) of a bit address.
pub const fn bit_mask(addr: u8) -> u8 {
    1 << (addr & 0x07)
}

/// `true` if `addr` is inside the bit-addressable RAM block (`0x20..=0x2F`).
pub const fn is_bit_ram(addr: u8) -> bool {
    addr >= BIT_RAM_BASE && addr < BIT_RAM_BASE + BIT_RAM_BYTES
}

/// `true` if the SFR at `addr` is bit addressable (its address is 8-aligned).
pub const fn is_bit_addressable_sfr(addr: u8) -> bool {
    addr >= 0x80 && (addr & 0x07) == 0
}

/// `true` if `addr` can be used as a bit operand.
pub const fn is_valid_bit(addr: u8) -> bool {
    addr < 0x80 || is_bit_addressable_sfr(addr)
}

/// The register bank (0..=3) that internal RAM address `addr` belongs to.
pub const fn register_bank(addr: u8) -> Option<u8> {
    if addr < 0x20 { Some(addr >> 3) } else { None }
}

/// Address of `Rn` in a given register bank.
pub const fn register_addr(bank: u8, n: u8) -> u8 {
    bank * 8 + (n & 0x07)
}

/// The default stack depth available after reset, in bytes.
///
/// The 8051 resets `SP` to `0x07`, so the first `PUSH` lands at `0x08` — inside
/// register bank 1. Kernel startup code normally moves `SP` up to `0x30` or
/// beyond, which this constant documents.
pub const RESET_SP: u8 = 0x07;
