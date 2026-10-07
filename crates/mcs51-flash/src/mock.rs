//! A simulated STC bootloader.
//!
//! This is the *device* side of the protocol: a byte-level model of what an STC
//! part does when it is sitting in its bootloader. It exists so the flasher can
//! be exercised end to end without a chip on the desk, and so the tests can
//! compare the bytes that actually landed in flash with the image that was
//! downloaded.
//!
//! It is deliberately permissive about timing and strict about framing and
//! checksums: exactly the parts of the protocol the host has to get right.

use std::collections::VecDeque;

use crate::frame::{self, Checksum};
use crate::stc::StcFamily;
use crate::transport::{Device, Parity};

/// A model of an STC microcontroller in bootloader mode.
#[derive(Debug, Clone)]
pub struct MockStc {
    family: StcFamily,
    /// Flash contents, indexed from address zero.
    pub flash: Vec<u8>,
    /// The option (fuse) byte the chip reports and accepts.
    pub option_byte: u8,
    /// The system clock the chip pretends to run at.
    pub clock_hz: f64,
    /// BSL version byte.
    pub bsl_version: u8,
    /// Model id in the status packet.
    pub magic: u16,
    /// Baud rate the chip is currently using.
    pub baud: u32,
    /// Parity setting.
    pub parity: Parity,
    /// Bytes queued for the host.
    out: VecDeque<u8>,
    /// Partially received host packet.
    input: Vec<u8>,
    /// `true` once the host has synchronised.
    pub synchronised: bool,
    /// `true` once the host asked the chip to run the program.
    pub running: bool,
    /// Every host payload received, for assertions.
    pub received: Vec<Vec<u8>>,
}

impl MockStc {
    /// A model of an STC89 part with `size` bytes of flash.
    pub fn stc89(size: usize) -> Self {
        Self::new(StcFamily::Stc89, size, 0xF002, 11_059_200.0)
    }

    /// A model of an STC12/15/8 part with `size` bytes of flash.
    pub fn stc12(size: usize) -> Self {
        Self::new(StcFamily::Stc12, size, 0xF047, 11_059_200.0)
    }

    /// Build a model.
    pub fn new(family: StcFamily, size: usize, magic: u16, clock_hz: f64) -> Self {
        Self {
            family,
            flash: vec![0xFFu8; size],
            // 0xFF means: 12-clock, no BSL pin detect, no EEPROM erase, low
            // gain, ALE on, XRAM off -- the factory default for an STC89.
            option_byte: 0xFF,
            clock_hz,
            bsl_version: 0x72,
            magic,
            baud: 2400,
            parity: Parity::None,
            out: VecDeque::new(),
            input: Vec::new(),
            synchronised: false,
            running: false,
            received: Vec::new(),
        }
    }

    fn checksum(&self) -> Checksum {
        match self.family {
            StcFamily::Stc89 => Checksum::Byte8,
            StcFamily::Stc12 => Checksum::Word16,
        }
    }

    fn reply(&mut self, payload: &[u8]) {
        let packet = frame::encode_mcu(payload, self.checksum());
        self.out.extend(packet);
    }

    /// The status packet the chip sends when it is synchronised.
    fn status_payload(&self) -> Vec<u8> {
        let mut payload = vec![0u8; 40];
        payload[0] = 0x00;
        // The chip reports how many of its own clocks fit in one bit time at
        // the handshake baud rate; eight samples are averaged by the host.
        let cpu_t = if self.family == StcFamily::Stc89 && (self.option_byte & 1) == 0 {
            6.0
        } else {
            12.0
        };
        let counter = (self.clock_hz * 7.0 / (self.baud as f64 * cpu_t)).round() as u16;
        for i in 0..8 {
            payload[1 + 2 * i] = (counter >> 8) as u8;
            payload[2 + 2 * i] = counter as u8;
        }
        payload[17] = self.bsl_version;
        payload[18] = b'D';
        payload[19] = self.option_byte;
        payload[20] = (self.magic >> 8) as u8;
        payload[21] = self.magic as u8;
        // The STC12+ status packet carries the option byte a second time.
        payload[23] = self.option_byte;
        payload[24] = 0x00;
        payload.truncate(25);
        payload
    }

    fn read_u16_be(payload: &[u8], at: usize) -> u16 {
        if payload.len() < at + 2 {
            return 0;
        }
        u16::from_be_bytes([payload[at], payload[at + 1]])
    }

    fn dispatch(&mut self, payload: Vec<u8>) {
        if payload.is_empty() {
            return;
        }
        self.received.push(payload.clone());
        let command = payload[0];
        match command {
            // Baud rate check and commit (STC89), or the STC12 variants.
            0x8F => {
                let brt = Self::read_u16_be(&payload, 1);
                let _ = brt;
                self.reply(&[0x8F]);
            }
            0x8E => {
                // The STC12+ dialects acknowledge a committed baud change with
                // an erase-status code instead of echoing the command.
                let reply = match self.family {
                    StcFamily::Stc89 => 0x8E,
                    StcFamily::Stc12 => 0x84,
                };
                self.reply(&[reply]);
            }
            0x50 => {
                // STC12 handshake start: the chip answers with the check code.
                self.reply(&[0x8F]);
            }
            // Ping-pong used to prove the new baud rate.
            0x80 => {
                let mut reply = vec![0x80];
                reply.extend_from_slice(&self.magic.to_be_bytes());
                self.reply(&reply);
            }
            // Erase.
            0x84 => {
                let status = match self.family {
                    StcFamily::Stc89 => {
                        let blocks = payload.get(1).copied().unwrap_or(0) as usize;
                        let bytes = blocks * 256;
                        let limit = bytes.min(self.flash.len());
                        self.flash[..limit].fill(0xFF);
                        0x80
                    }
                    StcFamily::Stc12 => {
                        // The countdown tail is a guard against accidental
                        // erasure; a real part refuses a malformed one.
                        let mut i = 0x80u8;
                        let mut countdown_ok = true;
                        for byte in payload.iter().skip(19) {
                            if *byte != i {
                                countdown_ok = false;
                                break;
                            }
                            if i == 0x0E {
                                break;
                            }
                            i -= 1;
                        }
                        if countdown_ok {
                            self.flash.fill(0xFF);
                        }
                        0x00
                    }
                };
                self.reply(&[status]);
            }
            // Write a block (STC89: command byte 0x00).
            0x00 if self.family == StcFamily::Stc89 => {
                let address = Self::read_u16_be(&payload, 3) as usize;
                let length = Self::read_u16_be(&payload, 5) as usize;
                let data = payload.get(7..7 + length).unwrap_or(&[]);
                let mut sum: u32 = 0;
                for (i, byte) in data.iter().enumerate() {
                    if address + i < self.flash.len() {
                        self.flash[address + i] = *byte;
                    }
                    sum += *byte as u32;
                }
                self.reply(&[0x80, (sum & 0xFF) as u8]);
            }
            // Write a block (STC12/15/8).
            0x22 | 0x02 if self.family == StcFamily::Stc12 => {
                let address = Self::read_u16_be(&payload, 1) as usize;
                // BSL 7.2 and later put a 5A A5 marker between the address and
                // the data.
                let start = if payload.get(3) == Some(&0x5A) && payload.get(4) == Some(&0xA5) {
                    5
                } else {
                    3
                };
                let data = payload.get(start..).unwrap_or(&[]);
                for (i, byte) in data.iter().enumerate() {
                    if address + i < self.flash.len() {
                        self.flash[address + i] = *byte;
                    }
                }
                self.reply(&[0x02, 0x54, 0x00]);
            }
            // Write finish (BSL 7.2 and later).
            0x07 => {
                self.reply(&[0x07, 0x54]);
            }
            // Option byte.
            0x8D => {
                if let Some(msr) = payload.get(1) {
                    self.option_byte = *msr;
                }
                self.reply(&[0x8D]);
            }
            // Run the program.
            0x82 => {
                self.running = true;
            }
            _ => {
                // Unknown command: a real part stays quiet, and so do we.
            }
        }
    }

    /// Every byte written into flash, as `(address, byte)`.
    pub fn written(&self) -> Vec<(u16, u8)> {
        self.flash
            .iter()
            .enumerate()
            .filter(|(_, byte)| **byte != 0xFF)
            .map(|(address, byte)| (address as u16, *byte))
            .collect()
    }
}

impl Device for MockStc {
    fn host_to_device(&mut self, byte: u8) {
        // A lone 0x7F outside a packet is the synchronisation pulse.
        if byte == 0x7F && self.input.is_empty() {
            self.synchronised = true;
            let payload = self.status_payload();
            self.reply(&payload);
            return;
        }
        self.input.push(byte);
        loop {
            match frame::parse_host(&self.input, self.checksum()) {
                None => return,
                Some(Ok((payload, consumed))) => {
                    self.input.drain(..consumed);
                    self.dispatch(payload);
                }
                Some(Err(_)) => {
                    // Drop one byte and try to resynchronise, the way a real
                    // BSL ignores noise on the line.
                    self.input.remove(0);
                    if self.input.is_empty() {
                        return;
                    }
                }
            }
        }
    }

    fn device_to_host(&mut self) -> Option<u8> {
        self.out.pop_front()
    }

    fn set_baud(&mut self, baud: u32) {
        self.baud = baud;
    }

    fn set_parity(&mut self, parity: Parity) {
        self.parity = parity;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_status_packet_decodes_to_the_configured_clock() {
        let chip = MockStc::stc89(8192);
        let payload = chip.status_payload();
        assert_eq!(payload[0], 0x00);
        let mut counter = 0u32;
        for i in 0..8 {
            counter += u16::from_be_bytes([payload[1 + 2 * i], payload[2 + 2 * i]]) as u32;
        }
        let counter = counter as f64 / 8.0;
        // This is the host's own formula.
        let clock = (2400.0 * counter * 12.0) / 7.0;
        assert!((clock - 11_059_200.0).abs() < 1000.0, "clock was {clock}");
        assert_eq!(u16::from_be_bytes([payload[20], payload[21]]), 0xF002);
    }

    #[test]
    fn a_sync_pulse_produces_a_framed_status_packet() {
        let mut chip = MockStc::stc89(8192);
        chip.host_to_device(0x7F);
        let mut out = Vec::new();
        while let Some(byte) = chip.device_to_host() {
            out.push(byte);
        }
        assert_eq!(&out[..3], &[0x46, 0xB9, 0x68]);
        assert_eq!(*out.last().unwrap(), 0x16);
        assert!(chip.synchronised);
    }
}
