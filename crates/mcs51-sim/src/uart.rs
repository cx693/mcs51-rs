//! The 8051 serial port.
//!
//! Modelled as an interrupt-driven byte pipe: writing `SBUF` starts a
//! transmission that completes `tx_cycles` machine cycles later (raising `TI`),
//! and bytes pushed in from the host appear in `SBUF` with `RI` set.
//!
//! Received and transmitted bytes are also collected into [`Uart::console`],
//! which is what the test harness asserts on.

use std::collections::VecDeque;

use mcs51_isa::sfr;

/// Serial port modes, from `SCON.SM0`/`SM1`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Mode 0: synchronous shift register.
    Shift,
    /// Mode 1: 8-bit UART, variable baud rate.
    Uart8,
    /// Mode 2: 9-bit UART, fixed baud rate.
    Uart9Fixed,
    /// Mode 3: 9-bit UART, variable baud rate.
    Uart9Variable,
}

impl Mode {
    /// Decode from `SCON`.
    pub fn from_scon(scon: u8) -> Mode {
        match (scon >> 6) & 0x03 {
            0 => Mode::Shift,
            1 => Mode::Uart8,
            2 => Mode::Uart9Fixed,
            _ => Mode::Uart9Variable,
        }
    }
}

/// The serial port state.
#[derive(Debug, Clone)]
pub struct Uart {
    /// `SCON`
    pub scon: u8,
    /// The transmit holding register, `SBUF` as seen by the CPU.
    pub sbuf: u8,
    /// Bytes that have left the wire.
    pub console: Vec<u8>,
    /// Bytes received but not yet delivered to `SBUF`.
    rx_queue: VecDeque<u8>,
    /// A transmission in flight: `(byte, cycles remaining)`.
    tx_pending: Option<(u8, u32)>,
    /// Machine cycles a byte takes to leave the port.
    pub tx_cycles: u32,
    /// Number of bytes transmitted.
    pub tx_count: u64,
    /// Number of bytes received.
    pub rx_count: u64,
    /// Writes to `SBUF` that were discarded because a byte was already in
    /// flight. Firmware that polls `TI` correctly never trips this.
    pub tx_dropped: u64,
    /// Drop received bytes when the queue is full instead of growing forever.
    pub rx_capacity: usize,
}

impl Default for Uart {
    fn default() -> Self {
        Self {
            scon: 0,
            sbuf: 0,
            console: Vec::new(),
            rx_queue: VecDeque::new(),
            tx_pending: None,
            // One 8051 machine cycle is 12 clocks; 10 bit-times at 9600 baud on
            // an 11.0592 MHz part is a little under 1000 machine cycles.
            tx_cycles: 96,
            tx_count: 0,
            rx_count: 0,
            tx_dropped: 0,
            rx_capacity: 256,
        }
    }
}

impl Uart {
    /// A UART that transmits instantly, for tests that only care about output.
    pub fn immediate() -> Self {
        Self {
            tx_cycles: 0,
            ..Self::default()
        }
    }

    /// The current mode.
    pub fn mode(&self) -> Mode {
        Mode::from_scon(self.scon)
    }

    /// `true` when the transmitter is empty.
    pub fn tx_ready(&self) -> bool {
        self.scon & SCON_TI != 0
    }

    /// `true` when a received byte is waiting.
    pub fn rx_ready(&self) -> bool {
        self.scon & SCON_RI != 0
    }

    /// `true` when the receiver has been enabled.
    pub fn receiver_enabled(&self) -> bool {
        self.scon & SCON_REN != 0
    }

    /// Queue a byte for transmission, as a `MOV SBUF, A` would.
    ///
    /// A byte already in flight is not disturbed: like the hardware, the
    /// previous transmission finishes first and the caller should have polled
    /// `TI`.
    pub fn write_sbuf(&mut self, value: u8) {
        if self.tx_pending.is_none() {
            self.tx_pending = Some((value, self.tx_cycles));
            self.scon &= !SCON_TI;
            if self.tx_cycles == 0 {
                self.finish_tx();
            }
        } else {
            self.tx_dropped += 1;
        }
    }

    fn finish_tx(&mut self) {
        if let Some((byte, _)) = self.tx_pending.take() {
            self.console.push(byte);
            self.tx_count += 1;
            self.scon |= SCON_TI;
        }
    }

    /// Hand a byte to the receiver as if it arrived on RXD.
    pub fn feed(&mut self, byte: u8) {
        if self.rx_queue.len() < self.rx_capacity {
            self.rx_queue.push_back(byte);
        }
    }

    /// Hand several bytes to the receiver.
    pub fn feed_bytes(&mut self, bytes: &[u8]) {
        for byte in bytes {
            self.feed(*byte);
        }
    }

    /// Advance the port by `cycles` machine cycles.
    pub fn tick(&mut self, cycles: u32) {
        if let Some((byte, remaining)) = self.tx_pending {
            if cycles >= remaining {
                self.tx_pending = Some((byte, 0));
                self.finish_tx();
            } else {
                self.tx_pending = Some((byte, remaining - cycles));
            }
        }
        if self.scon & SCON_RI == 0
            && self.receiver_enabled()
            && let Some(byte) = self.rx_queue.pop_front()
        {
            self.sbuf = byte;
            self.scon |= SCON_RI;
            self.rx_count += 1;
        }
    }

    /// Write `SCON`.
    pub fn write_scon(&mut self, value: u8) {
        // TI and RI are only ever set by hardware, so a software write of 0
        // clears them and a write of 1 leaves the hardware state alone.
        let preserved = self.scon & (SCON_TI | SCON_RI);
        self.scon = (value & !(SCON_TI | SCON_RI)) | (value & preserved);
    }

    /// Read the console as text, replacing non-printable bytes.
    pub fn console_string(&self) -> String {
        self.console
            .iter()
            .map(|b| {
                if (0x20..0x7F).contains(b) || *b == b'\n' || *b == b'\r' || *b == b'\t' {
                    *b as char
                } else {
                    '.'
                }
            })
            .collect()
    }
}

/// `SCON` bit masks.
pub const SCON_RI: u8 = 1 << 0;
/// `SCON.TI`
pub const SCON_TI: u8 = 1 << 1;
/// `SCON.RB8`
pub const SCON_RB8: u8 = 1 << 2;
/// `SCON.TB8`
pub const SCON_TB8: u8 = 1 << 3;
/// `SCON.REN`
pub const SCON_REN: u8 = 1 << 4;
/// `SCON.SM2`
pub const SCON_SM2: u8 = 1 << 5;
/// `SCON.SM1`
pub const SCON_SM1: u8 = 1 << 6;
/// `SCON.SM0`
pub const SCON_SM0: u8 = 1 << 7;

/// `true` when the SFR address belongs to the serial port.
pub fn owns(addr: u8) -> bool {
    matches!(addr, sfr::SFR_SCON | sfr::SFR_SBUF)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transmits_after_the_configured_delay() {
        let mut uart = Uart {
            tx_cycles: 10,
            ..Default::default()
        };
        uart.write_sbuf(b'A');
        assert!(uart.console.is_empty());
        assert!(!uart.tx_ready());
        uart.tick(9);
        assert!(uart.console.is_empty());
        uart.tick(1);
        assert_eq!(uart.console, b"A");
        assert!(uart.tx_ready());
    }

    #[test]
    fn immediate_mode_sets_ti_at_once() {
        let mut uart = Uart::immediate();
        uart.write_sbuf(b'Z');
        assert_eq!(uart.console, b"Z");
        assert!(uart.tx_ready());
    }

    #[test]
    fn receives_only_when_enabled_and_idle() {
        let mut uart = Uart::default();
        uart.feed(b'q');
        uart.tick(1);
        assert!(!uart.rx_ready(), "REN is clear");

        uart.write_scon(SCON_REN);
        uart.tick(1);
        assert!(uart.rx_ready());
        assert_eq!(uart.sbuf, b'q');

        // A second byte waits until RI is cleared.
        uart.feed(b'r');
        uart.tick(1);
        assert_eq!(uart.sbuf, b'q');
        uart.write_scon(SCON_REN);
        uart.tick(1);
        assert_eq!(uart.sbuf, b'r');
    }

    #[test]
    fn software_can_clear_ti_but_not_set_it() {
        let mut uart = Uart::immediate();
        uart.write_sbuf(b'x');
        assert!(uart.tx_ready());
        uart.write_scon(SCON_REN); // writing 0 to TI clears it
        assert!(!uart.tx_ready());
        uart.write_scon(SCON_REN | SCON_TI); // writing 1 does not set it
        assert!(!uart.tx_ready());
    }
}
