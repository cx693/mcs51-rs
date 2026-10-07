//! Special function registers and bit addresses.

/// First address of the SFR area; direct addresses `0x80..=0xFF` are SFRs.
pub const SFR_BASE: u8 = 0x80;

macro_rules! sfr_addrs {
    ($($konst:ident = $value:literal),* $(,)?) => {
        $(
            #[doc = concat!("`", stringify!($konst), "` -- SFR at address ", stringify!($value), ".")]
            pub const $konst: u8 = $value;
        )*
    };
}

sfr_addrs! {
    SFR_P0 = 0x80,
    SFR_SP = 0x81,
    SFR_DPL = 0x82,
    SFR_DPH = 0x83,
    SFR_PCON = 0x87,
    SFR_TCON = 0x88,
    SFR_TMOD = 0x89,
    SFR_TL0 = 0x8A,
    SFR_TL1 = 0x8B,
    SFR_TH0 = 0x8C,
    SFR_TH1 = 0x8D,
    SFR_P1 = 0x90,
    SFR_SCON = 0x98,
    SFR_SBUF = 0x99,
    SFR_P2 = 0xA0,
    SFR_IE = 0xA8,
    SFR_P3 = 0xB0,
    SFR_IP = 0xB8,
    SFR_T2CON = 0xC8,
    SFR_T2MOD = 0xC9,
    SFR_RCAP2L = 0xCA,
    SFR_RCAP2H = 0xCB,
    SFR_TL2 = 0xCC,
    SFR_TH2 = 0xCD,
    SFR_PSW = 0xD0,
    SFR_ACC = 0xE0,
    SFR_B = 0xF0,
}

/// `TCON.TR0` -- start timer 0.
pub const TCON_TR0: u8 = 1 << 4;
/// `TCON.TF0` -- timer 0 overflow flag.
pub const TCON_TF0: u8 = 1 << 5;
/// `TCON.TR1` -- start timer 1.
pub const TCON_TR1: u8 = 1 << 6;
/// `TCON.TF1` -- timer 1 overflow flag.
pub const TCON_TF1: u8 = 1 << 7;
/// `TCON.IT0` -- INT0 edge/level select.
pub const TCON_IT0: u8 = 1 << 0;
/// `TCON.IE0` -- INT0 edge flag.
pub const TCON_IE0: u8 = 1 << 1;
/// `TCON.IT1` -- INT1 edge/level select.
pub const TCON_IT1: u8 = 1 << 2;
/// `TCON.IE1` -- INT1 edge flag.
pub const TCON_IE1: u8 = 1 << 3;

/// `TMOD` timer 0 mode mask.
pub const TMOD_M0: u8 = 0x03;
/// `TMOD` timer 1 mode mask.
pub const TMOD_M1: u8 = 0x30;
/// `TMOD.C/T0` -- timer 0 counts T0 pin edges instead of machine cycles.
pub const TMOD_CT0: u8 = 1 << 2;
/// `TMOD.C/T1`
pub const TMOD_CT1: u8 = 1 << 6;
/// `TMOD.GATE0`
pub const TMOD_GATE0: u8 = 1 << 3;
/// `TMOD.GATE1`
pub const TMOD_GATE1: u8 = 1 << 7;

/// `IE.EX0`
pub const IE_EX0: u8 = 1 << 0;
/// `IE.ET0`
pub const IE_ET0: u8 = 1 << 1;
/// `IE.EX1`
pub const IE_EX1: u8 = 1 << 2;
/// `IE.ET1`
pub const IE_ET1: u8 = 1 << 3;
/// `IE.ES` -- serial port interrupt.
pub const IE_ES: u8 = 1 << 4;
/// `IE.ET2`
pub const IE_ET2: u8 = 1 << 5;
/// `IE.EA` -- global interrupt enable.
pub const IE_EA: u8 = 1 << 7;

/// `PCON.IDL` -- idle mode.
pub const PCON_IDL: u8 = 1 << 0;
/// `PCON.PD` -- power-down mode.
pub const PCON_PD: u8 = 1 << 1;

/// `PSW.CY`
pub const PSW_CY: u8 = 1 << 7;
/// `PSW.AC`
pub const PSW_AC: u8 = 1 << 6;
/// `PSW.F0`
pub const PSW_F0: u8 = 1 << 5;
/// `PSW.RS1`
pub const PSW_RS1: u8 = 1 << 4;
/// `PSW.RS0`
pub const PSW_RS0: u8 = 1 << 3;
/// `PSW.OV`
pub const PSW_OV: u8 = 1 << 2;
/// `PSW.P`
pub const PSW_P: u8 = 1 << 0;

macro_rules! sfr_names {
    ($($name:literal = $addr:literal),* $(,)?) => {
        /// Every named special function register, with its direct address.
        pub static SFR_NAMES: &[(&str, u8)] = &[$(($name, $addr)),*];
    };
}

sfr_names! {
    "P0"   = 0x80,
    "SP"   = 0x81,
    "DPL"  = 0x82,
    "DPH"  = 0x83,
    "PCON" = 0x87,
    "TCON" = 0x88,
    "TMOD" = 0x89,
    "TL0"  = 0x8A,
    "TL1"  = 0x8B,
    "TH0"  = 0x8C,
    "TH1"  = 0x8D,
    "P1"   = 0x90,
    "SCON" = 0x98,
    "SBUF" = 0x99,
    "P2"   = 0xA0,
    "IE"   = 0xA8,
    "P3"   = 0xB0,
    "IP"   = 0xB8,
    "T2CON"  = 0xC8,
    "T2MOD"  = 0xC9,
    "RCAP2L" = 0xCA,
    "RCAP2H" = 0xCB,
    "TL2"    = 0xCC,
    "TH2"    = 0xCD,
    "PSW"    = 0xD0,
    "ACC"    = 0xE0,
    "B"      = 0xF0,
}

macro_rules! bit_names {
    ($($name:literal = $addr:literal),* $(,)?) => {
        /// Every named bit-addressable location, with its bit address.
        pub static BIT_NAMES: &[(&str, u8)] = &[$(($name, $addr)),*];
    };
}

bit_names! {
    // Port 0
    "P0.0" = 0x80, "P0.1" = 0x81, "P0.2" = 0x82, "P0.3" = 0x83,
    "P0.4" = 0x84, "P0.5" = 0x85, "P0.6" = 0x86, "P0.7" = 0x87,
    // Timer/counter control
    "IT0" = 0x88, "IE0" = 0x89, "IT1" = 0x8A, "IE1" = 0x8B,
    "TR0" = 0x8C, "TF0" = 0x8D, "TR1" = 0x8E, "TF1" = 0x8F,
    // Port 1
    "P1.0" = 0x90, "P1.1" = 0x91, "P1.2" = 0x92, "P1.3" = 0x93,
    "P1.4" = 0x94, "P1.5" = 0x95, "P1.6" = 0x96, "P1.7" = 0x97,
    // Serial port control
    "RI"  = 0x98, "TI"  = 0x99, "RB8" = 0x9A, "TB8" = 0x9B,
    "REN" = 0x9C, "SM2" = 0x9D, "SM1" = 0x9E, "SM0" = 0x9F,
    // Port 2
    "P2.0" = 0xA0, "P2.1" = 0xA1, "P2.2" = 0xA2, "P2.3" = 0xA3,
    "P2.4" = 0xA4, "P2.5" = 0xA5, "P2.6" = 0xA6, "P2.7" = 0xA7,
    // Interrupt enable
    "EX0" = 0xA8, "ET0" = 0xA9, "EX1" = 0xAA, "ET1" = 0xAB,
    "ES"  = 0xAC, "ET2" = 0xAD, "EA"  = 0xAF,
    // Port 3 (with alternate functions)
    "RXD"  = 0xB0, "TXD"  = 0xB1, "INT0" = 0xB2, "INT1" = 0xB3,
    "T0"   = 0xB4, "T1"   = 0xB5, "WR"   = 0xB6, "RD"   = 0xB7,
    // Interrupt priority
    "PX0" = 0xB8, "PT0" = 0xB9, "PX1" = 0xBA, "PT1" = 0xBB,
    "PS"  = 0xBC, "PT2" = 0xBD,
    // Timer 2 control
    "CPRL2" = 0xC8, "C/T2" = 0xC9, "TR2" = 0xCA, "EXEN2" = 0xCB,
    "TCLK"  = 0xCC, "RCLK" = 0xCD, "EXF2" = 0xCE, "TF2"   = 0xCF,
    // Program status word
    "P"   = 0xD0, "OV"  = 0xD1, "RS0" = 0xD2, "RS1" = 0xD3,
    "F0"  = 0xD4, "AC"  = 0xD5, "CY"  = 0xD6,
}

/// Number of direct addresses (internal data + SFR).
pub const DIRECT_SPACE: usize = 256;

/// Look up an SFR name, case-insensitively.
pub fn lookup_direct(name: &str) -> Option<u8> {
    SFR_NAMES
        .iter()
        .find(|(n, _)| n.eq_ignore_ascii_case(name))
        .map(|(_, a)| *a)
}

/// Look up a bit name, case-insensitively.
pub fn lookup_bit(name: &str) -> Option<u8> {
    BIT_NAMES
        .iter()
        .find(|(n, _)| n.eq_ignore_ascii_case(name))
        .map(|(_, a)| *a)
}

/// The name of an SFR address, if it has one.
pub fn direct_name(addr: u8) -> Option<&'static str> {
    SFR_NAMES.iter().find(|(_, a)| *a == addr).map(|(n, _)| *n)
}

/// The name of a bit address, if it has one.
pub fn bit_name(addr: u8) -> Option<&'static str> {
    BIT_NAMES.iter().find(|(_, a)| *a == addr).map(|(n, _)| *n)
}

/// Format a direct address the way the disassembler prints it.
pub fn format_direct(addr: u8) -> DirectText {
    DirectText(addr)
}

/// Format a bit address the way the disassembler prints it.
pub fn format_bit(addr: u8) -> BitText {
    BitText(addr)
}

/// `Display` adapter produced by [`format_direct`].
#[derive(Debug, Clone, Copy)]
pub struct DirectText(pub u8);

impl core::fmt::Display for DirectText {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match direct_name(self.0) {
            Some(name) => f.write_str(name),
            None => write!(f, "0x{:02X}", self.0),
        }
    }
}

/// `Display` adapter produced by [`format_bit`].
#[derive(Debug, Clone, Copy)]
pub struct BitText(pub u8);

impl core::fmt::Display for BitText {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        if let Some(name) = bit_name(self.0) {
            return f.write_str(name);
        }
        if self.0 < 0x80 {
            write!(f, "0x{:02X}", self.0)
        } else {
            write!(f, "{}.{}", DirectText(self.0 & 0xF8), self.0 & 0x07)
        }
    }
}
