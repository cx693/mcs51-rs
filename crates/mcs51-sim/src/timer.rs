//! Timer/counter units.
//!
//! The 8051 has two 16-bit timer/counters driven from the SFR block. `TR0`/`TR1`
//! in `TCON` start them, `TMOD` selects mode and source, and overflow raises
//! `TF0`/`TF1`.

use mcs51_isa::sfr;

/// Which of the two timers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Which {
    /// Timer 0, interrupt vector `0x000B`.
    Timer0,
    /// Timer 1, interrupt vector `0x001B`.
    Timer1,
}

impl Which {
    const fn index(self) -> usize {
        match self {
            Which::Timer0 => 0,
            Which::Timer1 => 1,
        }
    }

    const fn run_bit(self) -> u8 {
        match self {
            Which::Timer0 => 0x10, // TR0
            Which::Timer1 => 0x40, // TR1
        }
    }

    const fn overflow_bit(self) -> u8 {
        match self {
            Which::Timer0 => 0x20, // TF0
            Which::Timer1 => 0x80, // TF1
        }
    }

    const fn mode_shift(self) -> u8 {
        match self {
            Which::Timer0 => 0,
            Which::Timer1 => 4,
        }
    }
}

/// One timer/counter's counters.
#[derive(Debug, Clone, Copy, Default)]
pub struct Timer {
    high: u8,
    low: u8,
    /// Number of overflows since reset.
    pub overflows: u64,
}

impl Timer {
    /// `THn`
    pub fn high(&self) -> u8 {
        self.high
    }

    /// `TLn`
    pub fn low(&self) -> u8 {
        self.low
    }

    /// Set `THn`.
    pub fn set_high(&mut self, value: u8) {
        self.high = value;
    }

    /// Set `TLn`.
    pub fn set_low(&mut self, value: u8) {
        self.low = value;
    }

    /// The 16-bit counter value (13-bit in mode 0).
    pub fn value(&self) -> u16 {
        ((self.high as u16) << 8) | self.low as u16
    }

    /// Prescaled advance. Returns the number of overflows that occurred.
    ///
    /// `mode` follows `TMOD`: 0 = 13-bit, 1 = 16-bit, 2 = 8-bit auto-reload,
    /// 3 = split (timer 0 only; treated as mode 1 here).
    pub fn advance(&mut self, mode: u8, cycles: u32) -> u64 {
        if cycles == 0 {
            return 0;
        }
        match mode & 0x03 {
            0 => {
                // 13-bit: TH holds the top 5 bits.
                let mut value = ((self.high as u32 & 0x1F) << 8) | self.low as u32;
                let period = 1u32 << 13;
                let total = value + cycles;
                let wraps = (total / period) as u64;
                value = total % period;
                self.high = ((value >> 8) & 0x1F) as u8;
                self.low = value as u8;
                self.overflows += wraps;
                wraps
            }
            1 | 3 => {
                let period = 1u32 << 16;
                let total = ((self.high as u32) << 8 | self.low as u32) + cycles;
                let wraps = (total / period) as u64;
                let value = total % period;
                self.high = (value >> 8) as u8;
                self.low = value as u8;
                self.overflows += wraps;
                wraps
            }
            _ => {
                // Mode 2: TL counts, TH reloads it.
                let start = self.low as u32;
                let total = start + cycles;
                let wraps = (total / 256) as u64;
                self.low = (total % 256) as u8;
                self.overflows += wraps;
                if wraps > 0 {
                    self.low = self.high;
                }
                wraps
            }
        }
    }
}

/// The block of timer state that lives behind the SFR interface.
#[derive(Debug, Clone, Copy, Default)]
pub struct Timers {
    /// `TCON`
    pub tcon: u8,
    /// `TMOD`
    pub tmod: u8,
    /// Timer 0.
    pub timer0: Timer,
    /// Timer 1.
    pub timer1: Timer,
}

impl Timers {
    /// The timer for `which`.
    pub fn get(&self, which: Which) -> &Timer {
        match which {
            Which::Timer0 => &self.timer0,
            Which::Timer1 => &self.timer1,
        }
    }

    /// The timer for `which`, mutably.
    pub fn get_mut(&mut self, which: Which) -> &mut Timer {
        match which {
            Which::Timer0 => &mut self.timer0,
            Which::Timer1 => &mut self.timer1,
        }
    }

    /// `true` when this timer is running as a timer (not an event counter).
    fn is_running(&self, which: Which) -> bool {
        let mode = self.mode(which);
        // In mode 3, timer 1 stops.
        if matches!(which, Which::Timer1) && self.mode(Which::Timer0) == 3 {
            return false;
        }
        let _ = mode;
        (self.tcon & which.run_bit()) != 0
    }

    /// `TMOD` mode field for a timer.
    pub fn mode(&self, which: Which) -> u8 {
        (self.tmod >> which.mode_shift()) & 0x03
    }

    /// `true` when the counter is clocked from the Tn pin instead of the
    /// machine cycle.
    pub fn is_counter(&self, which: Which) -> bool {
        let bit = match which {
            Which::Timer0 => 0x04, // C/T0
            Which::Timer1 => 0x40, // C/T1
        };
        (self.tmod & bit) != 0
    }

    /// `true` when `TFn` is set.
    pub fn overflow_flag(&self, which: Which) -> bool {
        (self.tcon & which.overflow_bit()) != 0
    }

    /// Clear `TFn`.
    pub fn clear_overflow(&mut self, which: Which) {
        self.tcon &= !which.overflow_bit();
    }

    /// Advance both timers by `cycles` machine cycles.
    pub fn tick(&mut self, cycles: u32) {
        for which in [Which::Timer0, Which::Timer1] {
            if !self.is_running(which) || self.is_counter(which) {
                continue;
            }
            let mode = self.mode(which);
            let wraps = match which {
                Which::Timer0 => self.timer0.advance(mode, cycles),
                Which::Timer1 => self.timer1.advance(mode, cycles),
            };
            if wraps > 0 {
                self.tcon |= which.overflow_bit();
            }
        }
    }

    /// Write one of the timer SFRs.
    pub fn write_sfr(&mut self, addr: u8, value: u8) -> bool {
        match addr {
            sfr::SFR_TCON => self.tcon = value,
            sfr::SFR_TMOD => self.tmod = value,
            sfr::SFR_TL0 => self.timer0.set_low(value),
            sfr::SFR_TH0 => self.timer0.set_high(value),
            sfr::SFR_TL1 => self.timer1.set_low(value),
            sfr::SFR_TH1 => self.timer1.set_high(value),
            _ => return false,
        }
        true
    }

    /// Read one of the timer SFRs, if it is one.
    pub fn read_sfr(&self, addr: u8) -> Option<u8> {
        Some(match addr {
            sfr::SFR_TCON => self.tcon,
            sfr::SFR_TMOD => self.tmod,
            sfr::SFR_TL0 => self.timer0.low(),
            sfr::SFR_TH0 => self.timer0.high(),
            sfr::SFR_TL1 => self.timer1.low(),
            sfr::SFR_TH1 => self.timer1.high(),
            _ => return None,
        })
    }

    /// `true` when the address belongs to this block.
    pub fn owns(addr: u8) -> bool {
        matches!(
            addr,
            sfr::SFR_TCON
                | sfr::SFR_TMOD
                | sfr::SFR_TL0
                | sfr::SFR_TH0
                | sfr::SFR_TL1
                | sfr::SFR_TH1
        )
    }
}

/// Helper used by the peripheral block to find a timer by interrupt name.
pub fn index_of(which: Which) -> usize {
    which.index()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mode1_counts_and_overflows() {
        let mut timer = Timer::default();
        timer.set_high(0xFF);
        timer.set_low(0xFE);
        assert_eq!(timer.advance(1, 1), 0);
        assert_eq!(timer.value(), 0xFFFF);
        assert_eq!(timer.advance(1, 1), 1);
        assert_eq!(timer.value(), 0x0000);
    }

    #[test]
    fn mode2_reloads_tl_from_th() {
        let mut timer = Timer::default();
        timer.set_high(0xF0);
        timer.set_low(0xFE);
        assert_eq!(timer.advance(2, 1), 0);
        assert_eq!(timer.low(), 0xFF);
        assert_eq!(timer.advance(2, 1), 1);
        assert_eq!(timer.low(), 0xF0, "TL must reload from TH");
    }

    #[test]
    fn mode0_is_thirteen_bits() {
        let mut timer = Timer::default();
        timer.set_high(0xFF);
        timer.set_low(0xFF);
        // Only the low 13 bits count, so 0x1FFF + 1 wraps.
        assert_eq!(timer.advance(0, 1), 1);
        assert_eq!(timer.value() & 0x1FFF, 0);
        assert_eq!(timer.high() & 0x1F, 0);
    }

    #[test]
    fn tmod_selects_mode_and_source() {
        let mut timers = Timers::default();
        timers.tmod = 0x02; // timer 0, mode 2, timer
        assert_eq!(timers.mode(Which::Timer0), 2);
        assert!(!timers.is_counter(Which::Timer0));
        timers.tmod = 0x06; // timer 0, mode 2, counter
        assert!(timers.is_counter(Which::Timer0));
    }
}
