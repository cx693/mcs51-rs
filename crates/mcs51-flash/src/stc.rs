//! The STC in-system-programming protocols.
//!
//! Two families share the packet framing in [`crate::frame`]:
//!
//! * [`StcFamily::Stc89`] — STC 89/90 series. No parity, one-byte checksum.
//! * [`StcFamily::Stc12`] — STC 10/11/12/15/8 series. Even parity, two-byte
//!   checksum, and an extra write-finish packet on BSL 7.2 and later.
//!
//! The sequences below are a Rust port of the ones used by the open-source
//! `stcflash` (a `stcgal` derivative) and `stcgal` tools, cross-checked against
//! the packet layouts they emit.

use std::thread::sleep;
use std::time::Duration;

use crate::frame::{self, Checksum, FrameError};
use crate::transport::{Parity, Transport};
use crate::{FlashError, Progress, Stage};

/// Which STC bootloader dialect to speak.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StcFamily {
    /// STC 89/90: `STC89C52RC` and friends.
    Stc89,
    /// STC 10/11/12/15/8: `STC12C5A60S2`, `STC15W4K32S4`, `STC8A8K64S4A12`.
    Stc12,
}

impl StcFamily {
    /// The CLI spelling.
    pub const fn as_str(self) -> &'static str {
        match self {
            StcFamily::Stc89 => "stc89",
            StcFamily::Stc12 => "stc12",
        }
    }

    /// Parse a family name, accepting the aliases the reference tools use.
    pub fn parse(text: &str) -> Option<StcFamily> {
        match text.to_ascii_lowercase().as_str() {
            "stc89" | "stc89a" | "stc90" | "stc" => Some(StcFamily::Stc89),
            "stc12" | "stc12a" | "stc12b" | "stc10" | "stc11" | "stc15" | "stc15a" | "stc8"
            | "stc8a" | "stc8d" | "stc8g" | "stc8h" => Some(StcFamily::Stc12),
            _ => None,
        }
    }

    const fn checksum(self) -> Checksum {
        match self {
            StcFamily::Stc89 => Checksum::Byte8,
            StcFamily::Stc12 => Checksum::Word16,
        }
    }

    /// The parity setting this dialect uses.
    pub const fn parity(self) -> Parity {
        match self {
            StcFamily::Stc89 => Parity::None,
            StcFamily::Stc12 => Parity::Even,
        }
    }

    /// Bytes per programming block.
    pub const fn block_size(self) -> usize {
        128
    }
}

/// How to put the chip into its bootloader.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResetMode {
    /// Do not touch the handshake lines: the operator cycles power by hand.
    Manual,
    /// Pulse DTR (most USB-TTL adapters).
    Dtr,
    /// Pulse RTS.
    Rts,
    /// Pulse the inverted DTR level.
    DtrInverted,
    /// Pulse the inverted RTS level.
    RtsInverted,
}

impl Default for ResetMode {
    fn default() -> Self {
        ResetMode::Manual
    }
}

/// Tunables for an STC download.
#[derive(Debug, Clone)]
pub struct StcOptions {
    /// Which dialect to speak.
    pub family: StcFamily,
    /// Speed used to find the chip. The BSL only listens at a fixed, slow rate.
    pub handshake_baud: u32,
    /// Speed used to transfer the image.
    pub transfer_baud: u32,
    /// How to reset the chip into its bootloader.
    pub reset: ResetMode,
    /// Write the option (fuse) byte back after programming.
    pub write_options: bool,
    /// Replacement option byte. `None` keeps whatever the chip reported, which
    /// is the safe default: it preserves the clock and reset settings the board
    /// was configured with.
    pub option_override: Option<u8>,
    /// Time to wait for the chip to answer the synchronisation pulse.
    ///
    /// The BSL only listens for a moment after a power-on reset, so this has to
    /// be long enough for a human to pull the power and plug it back in.
    pub sync_timeout: Duration,
}

impl Default for StcOptions {
    fn default() -> Self {
        Self {
            family: StcFamily::Stc89,
            handshake_baud: 2400,
            transfer_baud: 115_200,
            reset: ResetMode::Manual,
            write_options: true,
            option_override: None,
            // Long enough for a manual power cycle. The reference tools show a
            // countdown for the same reason.
            sync_timeout: Duration::from_secs(20),
        }
    }
}

/// What the chip told us about itself.
#[derive(Debug, Clone, PartialEq)]
pub struct StcInfo {
    /// The two-byte model id from the status packet.
    pub magic: u16,
    /// A best-effort model name decoded from the magic.
    pub model: String,
    /// BSL version, as the reference tool prints it (`7.2D`).
    pub bsl_version: String,
    /// BSL version as the raw byte.
    pub bsl_byte: u8,
    /// Measured system clock in Hz.
    pub clock_hz: f64,
    /// The option (fuse) byte the chip reported.
    pub option_byte: u8,
    /// `true` when the chip reports its 6-clock (double speed) mode.
    pub cpu_6t: bool,
}

impl StcInfo {
    /// The clock in MHz, for messages.
    pub fn clock_mhz(&self) -> f64 {
        self.clock_hz / 1e6
    }
}

/// A model name for an STC model id.
///
/// Only the common parts are listed; anything else is reported by magic so the
/// download still works, exactly as the reference tools do.
pub fn model_name(magic: u16) -> Option<&'static str> {
    Some(match magic {
        0xF001 => "STC89C51RC",
        0xF002 => "STC89C52RC",
        0xF003 => "STC89C53RC",
        0xF004 => "STC89C54RD+",
        0xF005 => "STC89C58RD+",
        0xF006 => "STC89C510RD+",
        0xF007 => "STC89C512RD+",
        0xF00A => "STC89LE51RC",
        0xF00B => "STC89LE52RC",
        0xF00C => "STC89LE54RD+",
        0xF00D => "STC89LE58RD+",
        0xF010 => "STC90C51RC",
        0xF011 => "STC90C52RC",
        0xF012 => "STC90C54RD+",
        0xF013 => "STC90C58RD+",
        0xF020 => "STC12C5201AD",
        0xF021 => "STC12C5202AD",
        0xF022 => "STC12C5204AD",
        0xF023 => "STC12C5206AD",
        0xF024 => "STC12C5404AD",
        0xF025 => "STC12C5406AD",
        0xF026 => "STC12C5408AD",
        0xF027 => "STC12C5410AD",
        0xF028 => "STC12C5412AD",
        0xF029 => "STC12C5201PWM",
        0xF02A => "STC12C5202PWM",
        0xF02B => "STC12C5204PWM",
        0xF02C => "STC12C5206PWM",
        0xF02D => "STC12C5608AD",
        0xF02E => "STC12C5616AD",
        0xF02F => "STC12C5620AD",
        0xF030 => "STC12C5624AD",
        0xF031 => "STC12C5628AD",
        0xF032 => "STC12C5632AD",
        0xF033 => "STC12C5201AD",
        0xF034 => "STC12C5201PWM",
        0xF035 => "STC12C5204AD",
        0xF036 => "STC12C5608AD",
        0xF037 => "STC12C5616AD",
        0xF038 => "STC12C5620AD",
        0xF039 => "STC12C5624AD",
        0xF03A => "STC12C5628AD",
        0xF03B => "STC12C5632AD",
        0xF040 => "STC12C5A08S2",
        0xF041 => "STC12C5A16S2",
        0xF042 => "STC12C5A32S2",
        0xF043 => "STC12C5A40S2",
        0xF044 => "STC12C5A48S2",
        0xF045 => "STC12C5A52S2",
        0xF046 => "STC12C5A56S2",
        0xF047 => "STC12C5A60S2",
        0xF048 => "STC12LE5A08S2",
        0xF049 => "STC12LE5A16S2",
        0xF04A => "STC12LE5A32S2",
        0xF04B => "STC12LE5A40S2",
        0xF04C => "STC12LE5A48S2",
        0xF04D => "STC12LE5A52S2",
        0xF04E => "STC12LE5A56S2",
        0xF04F => "STC12LE5A60S2",
        0xF050 => "STC15F104E",
        0xF051 => "STC15F204EA",
        0xF052 => "STC15F104W",
        0xF053 => "STC15F204EA",
        0xF054 => "STC15F2K60S2",
        0xF055 => "STC15F2K32S2",
        0xF056 => "STC15W408AS",
        0xF057 => "STC15W404AS",
        0xF058 => "STC15W401AS",
        0xF059 => "STC15W201S",
        0xF05A => "STC15W204S",
        0xF05B => "STC15W408S",
        0xF05C => "STC15W4K32S4",
        0xF05D => "STC15W4K40S4",
        0xF05E => "STC15W4K48S4",
        0xF05F => "STC15W4K56S4",
        0xF060 => "IAP15F2K61S2",
        0xF061 => "IAP15W4K58S4",
        0xF062 => "IAP15W4K61S4",
        0xF063 => "STC8A8K64S4A12",
        0xF064 => "STC8F2K64S4",
        0xF065 => "STC8F2K08S2",
        0xF066 => "STC8F1K08S2",
        0xF067 => "STC8A8K60S4A12",
        0xF068 => "STC8A4K64S2A12",
        0xF069 => "STC8A4K32S2A12",
        0xF06A => "STC8A4K16S2A12",
        _ => return None,
    })
}

/// An STC BSL session.
pub struct StcFlasher<T: Transport> {
    transport: T,
    options: StcOptions,
    info: Option<StcInfo>,
    /// Baud rate setting for the chip's baud-rate generator.
    brt: u16,
    brt_checksum: u8,
    iap_wait: u8,
    delay: u8,
}

impl<T: Transport> StcFlasher<T> {
    /// Wrap an open transport.
    pub fn new(transport: T, options: StcOptions) -> Self {
        Self {
            transport,
            options,
            info: None,
            brt: 0,
            brt_checksum: 0,
            iap_wait: 0,
            delay: 0,
        }
    }

    /// The session's transport.
    pub fn transport(&self) -> &T {
        &self.transport
    }

    /// What the chip reported, once connected.
    pub fn info(&self) -> Option<&StcInfo> {
        self.info.as_ref()
    }

    fn family(&self) -> StcFamily {
        self.options.family
    }

    fn checksum(&self) -> Checksum {
        self.family().checksum()
    }

    fn write_packet(&mut self, payload: &[u8]) -> Result<(), FlashError> {
        let packet = frame::encode_host(payload, self.checksum());
        self.transport.write_all(&packet)?;
        Ok(())
    }

    fn read_packet(&mut self) -> Result<Vec<u8>, FlashError> {
        let checksum = self.checksum();
        Ok(frame::read_packet(
            &mut self.transport,
            checksum,
            Duration::from_secs(10),
        )?)
    }

    /// Send the synchronisation byte until the chip answers with a status
    /// packet.
    ///
    /// The BSL is only listening for a moment after reset, so this repeats the
    /// pulse rather than sending it once.
    fn synchronise(&mut self, progress: &mut Progress) -> Result<Vec<u8>, FlashError> {
        let started = std::time::Instant::now();
        let deadline = started + self.options.sync_timeout;
        let mut last_error = None;
        let mut next_report = Duration::from_secs(1);
        let mut attempts = 0u32;

        while std::time::Instant::now() < deadline {
            let _ = self.transport.write_all(&[0x7F]);
            attempts += 1;
            sleep(Duration::from_millis(30));
            match self.transport.bytes_available() {
                Ok(0) => {
                    last_error = Some("no answer".to_string());
                }
                Ok(_) => match self.read_packet() {
                    Ok(packet) => {
                        progress.message(format!(
                            "芯片在第 {:.1} 秒应答（发了 {attempts} 次同步脉冲）",
                            started.elapsed().as_secs_f32()
                        ));
                        return Ok(packet);
                    }
                    Err(e) => last_error = Some(e.to_string()),
                },
                Err(e) => last_error = Some(e.to_string()),
            }

            // Show a countdown so the operator knows the window is still open.
            let elapsed = started.elapsed();
            if elapsed >= next_report {
                next_report += Duration::from_secs(1);
                let left = self.options.sync_timeout.saturating_sub(elapsed);
                progress.message(format!("还在等待芯片应答……剩余 {} 秒", left.as_secs()));
            }
        }

        Err(FlashError::Protocol(format!(
            "等了 {} 秒，芯片始终没有应答同步脉冲（{}）。\n\
             请确认：① 芯片确实断电重上电了；② TXD/RXD 没接反；\n\
             ③ 板子确实是 STC 且处于 ISP 模式；④ 用 --reset dtr 试试自动复位。",
            self.options.sync_timeout.as_secs(),
            last_error.unwrap_or_else(|| "timeout".into())
        )))
    }

    /// Reset the chip into its bootloader using the handshake lines.
    fn reset_into_bootloader(&mut self, progress: &mut Progress) -> Result<(), FlashError> {
        match self.options.reset.clone() {
            ResetMode::Manual => {
                progress.message("现在给板子断电，再重新上电（BSL 只在上电后短暂监听）");
            }
            mode => {
                let (dtr, rts) = match mode {
                    ResetMode::Dtr => (Some(true), None),
                    ResetMode::Rts => (None, Some(true)),
                    ResetMode::DtrInverted => (Some(false), None),
                    ResetMode::RtsInverted => (None, Some(false)),
                    ResetMode::Manual => unreachable!(),
                };
                let (dtr, rts) = match (dtr, rts) {
                    (Some(level), _) => (Some(level), None),
                    (_, Some(level)) => (None, Some(level)),
                    _ => (None, None),
                };
                if let Some(level) = dtr {
                    let _ = self.transport.set_dtr(level);
                }
                if let Some(level) = rts {
                    let _ = self.transport.set_rts(level);
                }
                sleep(Duration::from_millis(250));
                match mode {
                    ResetMode::Dtr | ResetMode::Rts => {
                        let _ = self.transport.set_dtr(false);
                        let _ = self.transport.set_rts(false);
                    }
                    ResetMode::DtrInverted | ResetMode::RtsInverted => {
                        let _ = self.transport.set_dtr(true);
                        let _ = self.transport.set_rts(true);
                    }
                    ResetMode::Manual => {}
                }
                sleep(Duration::from_millis(30));
            }
        }
        Ok(())
    }

    /// Find the chip and read its status packet.
    pub fn connect(&mut self, progress: &mut Progress) -> Result<StcInfo, FlashError> {
        progress.stage(Stage::Reset);
        self.transport.set_baud(self.options.handshake_baud)?;
        self.transport.set_parity(self.family().parity())?;
        let _ = self.transport.drain();
        self.reset_into_bootloader(progress)?;

        progress.stage(Stage::Handshake);
        let mut status = self.synchronise(progress)?;
        // Some BSLs ask to be acknowledged once more before they hand over the
        // real status packet.
        if status.first() == Some(&0x80) {
            self.transport.set_parity(Parity::Even)?;
            self.write_packet(&[0x80, 0x00, 0xF1])?;
            let _ = self.transport.write_all(&[0x7F]);
            status = self.read_packet()?;
        }
        let info = self.decode_status(&status)?;
        self.info = Some(info.clone());
        progress.info(&info);
        Ok(info)
    }

    fn decode_status(&mut self, status: &[u8]) -> Result<StcInfo, FlashError> {
        if status.len() < 23 {
            return Err(FlashError::Protocol(format!(
                "status packet is only {} bytes, expected at least 23",
                status.len()
            )));
        }
        let magic = u16::from_be_bytes([status[20], status[21]]);
        let bsl_byte = status[17];
        let bsl_version = format!(
            "{}.{}{}",
            bsl_byte >> 4,
            bsl_byte & 0x0F,
            status[18] as char
        );
        let option_byte = match self.family() {
            StcFamily::Stc89 => status[19],
            StcFamily::Stc12 => status.get(23).copied().unwrap_or(0xFF),
        };
        let cpu_6t = match self.family() {
            StcFamily::Stc89 => status[19] & 1 == 0,
            StcFamily::Stc12 => false,
        };

        // The chip reports how many of its own clock cycles fit into one bit
        // time at the handshake baud rate; eight samples are averaged.
        let mut counter: f64 = 0.0;
        for i in 0..8 {
            let hi = status[1 + 2 * i] as u16;
            let lo = status[2 + 2 * i] as u16;
            counter += u16::from_be_bytes([hi as u8, lo as u8]) as f64;
        }
        counter /= 8.0;
        let cpu_t = if cpu_6t { 6.0 } else { 12.0 };
        let clock_hz = (self.options.handshake_baud as f64 * counter * cpu_t) / 7.0;

        Ok(StcInfo {
            magic,
            model: model_name(magic)
                .map(str::to_string)
                .unwrap_or_else(|| format!("UNKNOWN ({magic:04X})")),
            bsl_version,
            bsl_byte,
            clock_hz,
            option_byte,
            cpu_6t,
        })
    }

    /// Work out the chip's baud-rate generator settings for the transfer speed.
    fn calculate_baud(&mut self) -> Result<(), FlashError> {
        let info = self
            .info
            .clone()
            .ok_or_else(|| FlashError::Protocol("not connected".into()))?;
        let clock = info.clock_hz;
        if clock <= 0.0 {
            return Err(FlashError::Protocol(
                "the chip reported a zero clock frequency".into(),
            ));
        }
        match self.family() {
            StcFamily::Stc89 => {
                let sample_rate = if info.cpu_6t { 16.0 } else { 32.0 };
                let target = self.options.transfer_baud as f64;
                let divisor = (clock / (target * sample_rate)).round();
                self.brt = (65536.0 - divisor) as u16;
                self.brt_checksum =
                    (2u32.wrapping_mul(256u32.wrapping_sub(self.brt as u32)) & 0xFF) as u8;
                self.iap_wait = if clock < 5e6 {
                    0x83
                } else if clock < 10e6 {
                    0x82
                } else if clock < 20e6 {
                    0x81
                } else {
                    0x80
                };
                self.delay = 0xA0;
            }
            StcFamily::Stc12 => {
                let target = self.options.transfer_baud as f64;
                let brt = 256.0 - (clock / (target * 16.0)).round();
                if brt <= 1.0 || brt > 255.0 {
                    return Err(FlashError::Protocol(format!(
                        "the chip cannot reach {} baud from {:.3} MHz",
                        self.options.transfer_baud,
                        clock / 1e6
                    )));
                }
                self.brt = brt as u16;
                self.brt_checksum =
                    (2u32.wrapping_mul(256u32.wrapping_sub(self.brt as u32)) & 0xFF) as u8;
                self.iap_wait = if clock < 1e6 {
                    0x87
                } else if clock < 2e6 {
                    0x86
                } else if clock < 3e6 {
                    0x85
                } else if clock < 6e6 {
                    0x84
                } else if clock < 12e6 {
                    0x83
                } else if clock < 20e6 {
                    0x82
                } else if clock < 24e6 {
                    0x81
                } else {
                    0x80
                };
                self.delay = 0x80;
            }
        }
        Ok(())
    }

    /// Switch the chip, and the host, to the transfer baud rate.
    pub fn handshake(&mut self, progress: &mut Progress) -> Result<(), FlashError> {
        self.calculate_baud()?;
        progress.stage(Stage::BaudSwitch);
        let magic = self.info.as_ref().map(|i| i.magic).unwrap_or(0);
        let brt_hi = (self.brt >> 8) as u8;
        let brt_lo = self.brt as u8;

        match self.family() {
            StcFamily::Stc89 => {
                // Ask the chip to check the new setting.
                let mut packet = vec![0x8F];
                packet.extend_from_slice(&self.brt.to_be_bytes());
                packet.push(0xFFu8.wrapping_sub(brt_hi));
                packet.push(self.brt_checksum);
                packet.push(self.delay);
                packet.push(self.iap_wait);
                self.write_packet(&packet)?;
                sleep(Duration::from_millis(100));
                self.transport.set_baud(self.options.transfer_baud)?;
                let reply = self.read_packet()?;
                self.transport.set_baud(self.options.handshake_baud)?;
                if reply.first() != Some(&0x8F) {
                    return Err(FlashError::Protocol(format!(
                        "baud check answered with {:02X?}, expected 8F",
                        reply.first()
                    )));
                }

                // Commit it.
                let mut packet = vec![0x8E];
                packet.extend_from_slice(&self.brt.to_be_bytes());
                packet.push(0xFFu8.wrapping_sub(brt_hi));
                packet.push(self.brt_checksum);
                packet.push(self.delay);
                self.write_packet(&packet)?;
                sleep(Duration::from_millis(100));
                self.transport.set_baud(self.options.transfer_baud)?;
                let reply = self.read_packet()?;
                if reply.first() != Some(&0x8E) {
                    return Err(FlashError::Protocol(format!(
                        "baud switch answered with {:02X?}, expected 8E",
                        reply.first()
                    )));
                }

                // Ping-pong to prove the new rate works in both directions.
                let mut ping = vec![0x80, 0x00, 0x00, 0x36, 0x01];
                ping.extend_from_slice(&magic.to_be_bytes());
                for _ in 0..4 {
                    self.write_packet(&ping)?;
                    let reply = self.read_packet()?;
                    if reply.first() != Some(&0x80) {
                        return Err(FlashError::Protocol(format!(
                            "baud test answered with {:02X?}, expected 80",
                            reply.first()
                        )));
                    }
                }
            }
            StcFamily::Stc12 => {
                // Start the handshake.
                let mut packet = vec![0x50, 0x00, 0x00, 0x36, 0x01];
                packet.extend_from_slice(&magic.to_be_bytes());
                self.write_packet(&packet)?;
                let reply = self.read_packet()?;
                if reply.first() != Some(&0x8F) {
                    return Err(FlashError::Protocol(format!(
                        "handshake answered with {:02X?}, expected 8F",
                        reply.first()
                    )));
                }

                // Test the new setting.
                self.write_packet(&[
                    0x8F,
                    0xC0,
                    brt_lo,
                    0x3F,
                    self.brt_checksum,
                    self.delay,
                    self.iap_wait,
                ])?;
                sleep(Duration::from_millis(100));
                self.transport.set_baud(self.options.transfer_baud)?;
                let reply = self.read_packet()?;
                self.transport.set_baud(self.options.handshake_baud)?;
                if reply.first() != Some(&0x8F) {
                    return Err(FlashError::Protocol(format!(
                        "baud test answered with {:02X?}, expected 8F",
                        reply.first()
                    )));
                }

                // Commit it.
                self.write_packet(&[0x8E, 0xC0, brt_lo, 0x3F, self.brt_checksum, self.delay])?;
                sleep(Duration::from_millis(100));
                self.transport.set_baud(self.options.transfer_baud)?;
                let reply = self.read_packet()?;
                if reply.first() != Some(&0x84) {
                    return Err(FlashError::Protocol(format!(
                        "baud switch answered with {:02X?}, expected 84",
                        reply.first()
                    )));
                }
            }
        }
        Ok(())
    }

    /// Erase enough flash for an image of `size` bytes.
    pub fn erase(&mut self, size: usize, progress: &mut Progress) -> Result<(), FlashError> {
        progress.stage(Stage::Erase);
        let blocks = size.div_ceil(512) * 2;
        progress.begin(Stage::Erase, blocks);
        progress.message(format!(
            "发送擦除命令（{blocks} 块，覆盖 {} 字节）",
            blocks * 256
        ));
        match self.family() {
            StcFamily::Stc89 => {
                let mut packet = vec![0x84, blocks.min(255) as u8];
                packet.extend_from_slice(&[0x33; 6]);
                self.write_packet(&packet)?;
                let reply = self.read_packet()?;
                if reply.first() != Some(&0x80) {
                    return Err(FlashError::Protocol(format!(
                        "erase answered with {:02X?}, expected 80",
                        reply.first()
                    )));
                }
            }
            StcFamily::Stc12 => {
                let full = size.div_ceil(512) * 2;
                let mut packet = vec![
                    0x84,
                    0xFF,
                    0x00,
                    blocks.min(255) as u8,
                    0x00,
                    0x00,
                    full.min(0xFFFF) as u8,
                    (full >> 8) as u8,
                ];
                packet.extend_from_slice(&[0x00; 11]);
                // The chip counts down a magic sequence as a final guard
                // against erasing by accident.
                let mut i = 0x80u8;
                while i > 0x0D {
                    packet.push(i);
                    i -= 1;
                }
                self.write_packet(&packet)?;
                let reply = self.read_packet()?;
                if reply.first() != Some(&0x00) {
                    return Err(FlashError::Protocol(format!(
                        "erase answered with {:02X?}, expected 00",
                        reply.first()
                    )));
                }
            }
        }
        progress.message("擦除完成".to_string());
        progress.advance(Stage::Erase, blocks);
        Ok(())
    }

    /// Write the image.
    pub fn program(&mut self, data: &[u8], progress: &mut Progress) -> Result<(), FlashError> {
        progress.stage(Stage::Program);
        progress.begin(Stage::Program, data.len());
        let block = self.family().block_size();
        let bsl = self.info.as_ref().map(|i| i.bsl_byte).unwrap_or(0);

        for (index, chunk) in data.chunks(block).enumerate() {
            let address = index * block;
            let mut payload = Vec::with_capacity(block + 8);
            match self.family() {
                StcFamily::Stc89 => {
                    payload.extend_from_slice(&[0x00, 0x00, 0x00]);
                    payload.extend_from_slice(&(address as u16).to_be_bytes());
                    payload.extend_from_slice(&(block as u16).to_be_bytes());
                    payload.extend_from_slice(chunk);
                    // Pad the final block with erased bytes rather than zeros,
                    // so flash past the end of the image reads back the way an
                    // unprogrammed part does.
                    while payload.len() < block + 7 {
                        payload.push(0xFF);
                    }
                }
                StcFamily::Stc12 => {
                    payload.push(if index == 0 { 0x22 } else { 0x02 });
                    payload.extend_from_slice(&(address as u16).to_be_bytes());
                    if bsl >= 0x72 {
                        payload.extend_from_slice(&[0x5A, 0xA5]);
                    }
                    payload.extend_from_slice(chunk);
                    while payload.len() < block + 3 {
                        payload.push(0xFF);
                    }
                }
            }
            self.write_packet(&payload)?;
            let reply = self.read_packet().map_err(|error| {
                FlashError::Protocol(format!(
                    "第 {index} 块（地址 {address:#06X}，{} 字节）没有收到应答：{error}",
                    chunk.len()
                ))
            })?;
            match self.family() {
                StcFamily::Stc89 => {
                    if reply.first() != Some(&0x80) {
                        return Err(FlashError::Protocol(format!(
                            "block {index} answered with {:02X?}, expected 80",
                            reply.first()
                        )));
                    }
                    // The chip checksums the whole 128-byte block, padding
                    // included, so compute it over the payload we just sent.
                    let expected: u32 = payload[7..].iter().map(|b| *b as u32).sum::<u32>() & 0xFF;
                    if reply.get(1).map(|b| *b as u32) != Some(expected) {
                        return Err(FlashError::Protocol(format!(
                            "block {index} failed its checksum: chip said {:02X?}, expected {expected:02X}",
                            reply.get(1)
                        )));
                    }
                }
                StcFamily::Stc12 => {
                    if reply.first() != Some(&0x02) || reply.get(1) != Some(&0x54) {
                        return Err(FlashError::Protocol(format!(
                            "block {index} answered with {:02X?}, expected 02 54",
                            &reply[..reply.len().min(2)]
                        )));
                    }
                    if let Some(last) = reply.get(2)
                        && *last != 0x00
                    {
                        return Err(FlashError::Protocol(format!(
                            "block {index} reported a write error ({last:#04X})"
                        )));
                    }
                }
            }
            progress.advance(Stage::Program, chunk.len());
        }
        progress.message(format!(
            "写入完成：{} 字节 / {} 块",
            data.len(),
            data.len().div_ceil(block)
        ));

        // BSL 7.2 and later want a write-finish packet.
        if self.family() == StcFamily::Stc12 && bsl >= 0x72 {
            self.write_packet(&[0x07, 0x00, 0x00, 0x5A, 0xA5])?;
            let reply = self.read_packet()?;
            if reply.first() != Some(&0x07) || reply.get(1) != Some(&0x54) {
                return Err(FlashError::Protocol(format!(
                    "write finish answered with {:02X?}, expected 07 54",
                    &reply[..reply.len().min(2)]
                )));
            }
        }
        Ok(())
    }

    /// Write the option (fuse) byte.
    pub fn program_options(&mut self, progress: &mut Progress) -> Result<(), FlashError> {
        if !self.options.write_options {
            return Ok(());
        }
        let info = self
            .info
            .clone()
            .ok_or_else(|| FlashError::Protocol("not connected".into()))?;
        let msr = self.options.option_override.unwrap_or(info.option_byte);
        progress.stage(Stage::Options);
        progress.message(format!("写回选项字节 {msr:#04X}"));
        let mut payload = vec![0x8D, msr, 0xFF, 0xFF, 0xFF];
        payload.truncate(match self.family() {
            StcFamily::Stc89 => 5,
            StcFamily::Stc12 => 5,
        });
        self.write_packet(&payload)?;
        let reply = self.read_packet()?;
        if reply.first() != Some(&0x8D) {
            return Err(FlashError::Protocol(format!(
                "option write answered with {:02X?}, expected 8D",
                reply.first()
            )));
        }
        Ok(())
    }

    /// Run the newly written program.
    pub fn disconnect(&mut self) -> Result<(), FlashError> {
        self.write_packet(&[0x82])?;
        let _ = self.transport.drain();
        Ok(())
    }
}

/// One decoded option (fuse) byte from an STC 89/90 part.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Stc89Options {
    /// The raw byte.
    pub raw: u8,
}

impl Stc89Options {
    /// Decode a raw option byte.
    ///
    /// The bit meanings follow the reference tool's `Stc89Option` class, which
    /// in turn came from the vendor's ISP utility.
    pub const fn new(raw: u8) -> Self {
        Self { raw }
    }

    /// `true` when the part runs in 6-clock (double speed) mode.
    pub const fn cpu_6t(self) -> bool {
        self.raw & 0x01 == 0
    }

    /// `true` when the BSL waits for a pin before running the application.
    pub const fn bsl_pin_detect(self) -> bool {
        self.raw & 0x04 == 0
    }

    /// `true` when the data flash is erased together with the program.
    pub const fn eeprom_erase(self) -> bool {
        self.raw & 0x08 == 0
    }

    /// Oscillator gain, `true` for high.
    pub const fn clock_gain_high(self) -> bool {
        self.raw & 0x10 != 0
    }

    /// `true` when the ALE pin keeps working (the bit means *enabled*, as in
    /// the reference tool).
    pub const fn ale_enabled(self) -> bool {
        self.raw & 0x20 != 0
    }

    /// `true` when external RAM access is enabled.
    pub const fn xram_enabled(self) -> bool {
        self.raw & 0x40 != 0
    }

    /// `true` when the power-on watchdog is armed. The bit is active low.
    pub const fn watchdog_enabled(self) -> bool {
        self.raw & 0x80 == 0
    }
}

impl std::fmt::Display for Stc89Options {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{:#04X} ({}-clock, BSL pin detect {}, EEPROM erase {}, clock gain {}, ALE {}, \
             XRAM {}, watchdog {})",
            self.raw,
            if self.cpu_6t() { 6 } else { 12 },
            if self.bsl_pin_detect() { "on" } else { "off" },
            if self.eeprom_erase() { "on" } else { "off" },
            if self.clock_gain_high() {
                "high"
            } else {
                "low"
            },
            if self.ale_enabled() { "on" } else { "off" },
            if self.xram_enabled() { "on" } else { "off" },
            if self.watchdog_enabled() { "on" } else { "off" }
        )
    }
}

impl From<FrameError> for FlashError {
    fn from(error: FrameError) -> Self {
        FlashError::Protocol(error.to_string())
    }
}
