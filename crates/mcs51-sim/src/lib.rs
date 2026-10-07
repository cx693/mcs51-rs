//! # mcs51-sim
//!
//! A complete 8051 *system* simulator: the CPU core comes from the independent
//! [`i8051`] emulator crate, and this crate supplies everything around it that
//! firmware actually talks to — the SFR block, two timer/counters, the serial
//! port, the I/O ports, and instruction-accurate cycle accounting.
//!
//! The point is to be able to *run* firmware produced by the `mcs51-rs`
//! toolchain and assert on what it does, with no hardware and no vendor
//! simulator in the loop:
//!
//! ```
//! use mcs51_sim::Machine;
//!
//! // MOV SBUF, #0x41 ; MOV SCON, #0x02  (transmit 'A', then set TI)
//! let code = vec![0x75, 0x99, 0x41, 0x75, 0x98, 0x42, 0x80, 0xFE];
//! let mut machine = Machine::with_uart_delay(code, 0);
//! machine.run(1000);
//! assert_eq!(machine.console(), b"A");
//! ```

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod timer;
pub mod uart;

use std::collections::VecDeque;

use i8051::memory::RAM;
use i8051::{Cpu, CpuView, PortMapper, ReadOnlyMemoryMapper};
use mcs51_ihex::Image;
use mcs51_isa::sfr;
use mcs51_isa::{Instruction, decode};

pub use timer::{Timer, Timers, Which};
pub use uart::{SCON_REN, SCON_RI, SCON_TI, Uart};

/// A write that the peripheral block applies when the CPU commits it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SfrWrite {
    /// SFR address.
    pub addr: u8,
    /// Value written.
    pub value: u8,
}

/// Everything on the 8051 die that is not the CPU core or RAM.
#[derive(Debug, Clone)]
pub struct Peripherals {
    sfr: [u8; 128],
    /// Port output latches, `P0`..`P3`.
    latches: [u8; 4],
    /// Values driven onto the pins from outside.
    pins: [u8; 4],
    /// Timer/counter block.
    pub timers: Timers,
    /// Serial port.
    pub uart: Uart,
    /// Every port write, for tracing.
    pub port_log: Vec<(u8, u8)>,
    /// Record port writes in [`Peripherals::port_log`].
    pub log_ports: bool,
    /// `PCON.IDL` was set: the CPU is asleep but peripherals keep running.
    pub idle: bool,
    /// `PCON.PD` was set: everything is stopped until reset.
    pub power_down: bool,
    /// Number of `MOVX` writes to external memory.
    pub xdata_writes: u64,
}

impl Default for Peripherals {
    fn default() -> Self {
        let mut sfr = [0u8; 128];
        // Port latches default high (pins pulled up), P2 defaults to 0 so that
        // `MOVX @Ri` addresses page zero unless firmware says otherwise.
        let latches = [0xFF, 0xFF, 0x00, 0xFF];
        sfr[(sfr::SFR_P0 - 0x80) as usize] = latches[0];
        sfr[(sfr::SFR_P1 - 0x80) as usize] = latches[1];
        sfr[(sfr::SFR_P2 - 0x80) as usize] = latches[2];
        sfr[(sfr::SFR_P3 - 0x80) as usize] = latches[3];
        Self {
            sfr,
            latches,
            pins: [0xFF; 4],
            timers: Timers::default(),
            uart: Uart::default(),
            port_log: Vec::new(),
            log_ports: false,
            idle: false,
            power_down: false,
            xdata_writes: 0,
        }
    }
}

impl Peripherals {
    /// Read an SFR without going through the CPU view.
    pub fn peek(&self, addr: u8) -> u8 {
        if let Some(value) = self.timers.read_sfr(addr) {
            return value;
        }
        match addr {
            sfr::SFR_SCON => self.uart.scon,
            sfr::SFR_SBUF => self.uart.sbuf,
            _ => match Self::port_index(addr) {
                Some(index) => self.latches[index] & self.pins[index],
                None => self.sfr[(addr & 0x7F) as usize],
            },
        }
    }

    /// Write an SFR without going through the CPU view.
    pub fn poke(&mut self, addr: u8, value: u8) {
        self.apply(SfrWrite { addr, value });
    }

    /// Drive the external pins of a port (`0`..`3`).
    pub fn set_pins(&mut self, port: usize, value: u8) {
        if port < 4 {
            self.pins[port] = value;
        }
    }

    /// The output latch of a port.
    pub fn latch(&self, port: usize) -> u8 {
        self.latches[port.min(3)]
    }

    /// Which of `P0`..`P3` an SFR address is, if any.
    ///
    /// This must be an exact match: `P0` is `0x80` and `P3` is `0xB0`, so a
    /// range pattern would swallow `SP`, `DPL`, `PCON`, `TCON`, `SCON` and the
    /// rest of the SFR block.
    fn port_index(addr: u8) -> Option<usize> {
        match addr {
            sfr::SFR_P0 => Some(0),
            sfr::SFR_P1 => Some(1),
            sfr::SFR_P2 => Some(2),
            sfr::SFR_P3 => Some(3),
            _ => None,
        }
    }

    fn apply(&mut self, write: SfrWrite) {
        let SfrWrite { addr, value } = write;
        match addr {
            sfr::SFR_SBUF => {
                self.uart.write_sbuf(value);
                self.sfr[(addr & 0x7F) as usize] = value;
            }
            sfr::SFR_SCON => {
                self.uart.write_scon(value);
                self.sfr[(addr & 0x7F) as usize] = self.uart.scon;
            }
            sfr::SFR_PCON => {
                self.sfr[(addr & 0x7F) as usize] = value;
                self.idle = value & 0x01 != 0;
                self.power_down = value & 0x02 != 0;
            }
            _ if Timers::owns(addr) => {
                self.timers.write_sfr(addr, value);
                self.sfr[(addr & 0x7F) as usize] = value;
            }
            _ => {
                if let Some(index) = Self::port_index(addr) {
                    self.latches[index] = value;
                    if self.log_ports {
                        self.port_log.push((addr, value));
                    }
                }
                self.sfr[(addr & 0x7F) as usize] = value;
            }
        }
    }

    /// Advance the peripherals by `cycles` machine cycles.
    pub fn tick(&mut self, cycles: u32) {
        if cycles == 0 {
            return;
        }
        self.timers.tick(cycles);
        self.uart.tick(cycles);
    }

    /// `true` when any enabled interrupt source is asserting.
    ///
    /// `IE` lives in the CPU core, not in the SFR block, so it is passed in.
    pub fn interrupt_pending(&self, ie: u8) -> bool {
        if ie & 0x80 == 0 {
            return false;
        }
        let timer0 = self.timers.overflow_flag(Which::Timer0) && ie & 0x02 != 0;
        let timer1 = self.timers.overflow_flag(Which::Timer1) && ie & 0x08 != 0;
        let serial = ie & 0x10 != 0 && (self.uart.scon & (SCON_TI | SCON_RI)) != 0;
        timer0 || timer1 || serial
    }
}

impl PortMapper for Peripherals {
    type WriteValue = SfrWrite;

    fn interest<C: CpuView>(&self, _cpu: &C, addr: u8) -> bool {
        addr >= 0x80
    }

    fn extend_short_read<C: CpuView>(&self, _cpu: &C, addr: u8) -> u16 {
        addr as u16 | (self.latches[2] as u16) << 8
    }

    fn read<C: CpuView>(&self, _cpu: &C, addr: u8) -> u8 {
        self.peek(addr)
    }

    fn read_latch<C: CpuView>(&self, _cpu: &C, addr: u8) -> u8 {
        match Self::port_index(addr) {
            Some(index) => self.latches[index],
            None => self.peek(addr),
        }
    }

    fn prepare_write<C: CpuView>(&self, _cpu: &C, addr: u8, value: u8) -> SfrWrite {
        SfrWrite { addr, value }
    }

    fn write(&mut self, value: SfrWrite) {
        self.apply(value);
    }
}

/// Program memory that reads as erased (`0xFF`) outside the loaded image.
#[derive(Debug, Clone)]
pub struct Rom {
    data: Vec<u8>,
}

impl Rom {
    /// Build a 64 KiB ROM from a sparse image.
    pub fn from_image(image: &Image) -> Self {
        let mut data = vec![0xFFu8; 0x10000];
        for (addr, byte) in image.iter() {
            data[addr as usize] = byte;
        }
        Self { data }
    }

    /// Build a ROM from a dense slice placed at address 0.
    pub fn new(data: Vec<u8>) -> Self {
        let mut rom = vec![0xFFu8; 0x10000];
        let len = data.len().min(0x10000);
        rom[..len].copy_from_slice(&data[..len]);
        Self { data: rom }
    }

    /// The raw bytes.
    pub fn as_slice(&self) -> &[u8] {
        &self.data
    }

    /// One byte of program memory.
    pub fn read_at(&self, addr: u16) -> u8 {
        self.data[addr as usize]
    }
}

impl ReadOnlyMemoryMapper for Rom {
    fn len(&self) -> u32 {
        0x10000
    }
    fn read<C: CpuView>(&self, _cpu: &C, addr: u32) -> u8 {
        self.data.get(addr as usize).copied().unwrap_or(0xFF)
    }
}

/// Why a run stopped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopReason {
    /// The program reached a self-jump (`SJMP $`), the conventional halt.
    Halted,
    /// `PCON.PD` was set.
    PowerDown,
    /// The instruction budget ran out.
    StepLimit,
    /// A caller-supplied predicate became true.
    Predicate,
}

/// One recorded instruction, for tracing.
#[derive(Debug, Clone)]
pub struct TraceEntry {
    /// Machine cycles retired before this instruction.
    pub cycles: u64,
    /// Program counter.
    pub pc: u16,
    /// Decoded instruction.
    pub instruction: Instruction,
    /// Accumulator before execution.
    pub a: u8,
}

/// A snapshot of the machine, for assertions and the debugger.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snapshot {
    /// Program counter.
    pub pc: u16,
    /// Accumulator.
    pub a: u8,
    /// B register.
    pub b: u8,
    /// Data pointer.
    pub dptr: u16,
    /// Stack pointer.
    pub sp: u8,
    /// PSW.
    pub psw: u8,
    /// Machine cycles retired.
    pub cycles: u64,
    /// Instructions retired.
    pub instructions: u64,
    /// Bytes transmitted on the serial port.
    pub tx_bytes: u64,
    /// Timer 0 counter.
    pub timer0: u16,
    /// Timer 1 counter.
    pub timer1: u16,
    /// `TCON`.
    pub tcon: u8,
}

/// A simulated 8051 system.
///
/// The three memory-mapped components are stored as one tuple because that is
/// what the `i8051` crate's [`CpuContext`] is implemented for.
pub struct Machine {
    /// The CPU core from the `i8051` crate.
    cpu: Cpu,
    /// `(ports and SFRs, external data memory, program memory)`.
    ctx: (Peripherals, RAM, Rom),
    /// Machine cycles retired.
    pub cycles: u64,
    /// Instructions retired.
    pub instructions: u64,
    /// Most recent instructions, when tracing is on.
    pub trace: VecDeque<TraceEntry>,
    /// How many instructions to keep in [`Machine::trace`].
    pub trace_limit: usize,
    /// Treat a self-jump as a halt.
    pub halt_on_self_jump: bool,
}

impl std::fmt::Debug for Machine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Machine")
            .field("pc", &format_args!("{:#06X}", self.pc()))
            .field("a", &format_args!("{:#04X}", self.cpu.a()))
            .field("cycles", &self.cycles)
            .field("instructions", &self.instructions)
            .finish()
    }
}

impl Machine {
    /// Build a machine around a dense program image starting at address 0.
    pub fn new(code: Vec<u8>) -> Self {
        Self::from_parts(Rom::new(code), Peripherals::default(), RAM::new())
    }

    /// Build a machine from a linked image.
    pub fn from_image(image: &Image) -> Self {
        Self::from_parts(Rom::from_image(image), Peripherals::default(), RAM::new())
    }

    /// Build a machine with the serial port set to transmit instantly.
    pub fn with_uart_delay(code: Vec<u8>, tx_cycles: u32) -> Self {
        let mut machine = Self::new(code);
        machine.ctx.0.uart.tx_cycles = tx_cycles;
        machine
    }

    /// Build a machine from explicit parts.
    pub fn from_parts(rom: Rom, peripherals: Peripherals, xdata: RAM) -> Self {
        Self {
            cpu: Cpu::new(),
            ctx: (peripherals, xdata, rom),
            cycles: 0,
            instructions: 0,
            trace: VecDeque::new(),
            trace_limit: 0,
            halt_on_self_jump: true,
        }
    }

    /// The CPU core.
    pub fn cpu(&self) -> &Cpu {
        &self.cpu
    }

    /// The SFR and peripheral block.
    pub fn peripherals(&self) -> &Peripherals {
        &self.ctx.0
    }

    /// The SFR and peripheral block, mutably.
    pub fn peripherals_mut(&mut self) -> &mut Peripherals {
        &mut self.ctx.0
    }

    /// Program memory.
    pub fn rom(&self) -> &Rom {
        &self.ctx.2
    }

    /// External data memory.
    pub fn xdata(&self) -> &RAM {
        &self.ctx.1
    }

    /// External data memory, mutably.
    pub fn xdata_mut(&mut self) -> &mut RAM {
        &mut self.ctx.1
    }

    /// Program counter.
    pub fn pc(&self) -> u16 {
        self.cpu.pc_ext(&self.ctx) as u16
    }

    /// Read an SFR.
    pub fn sfr(&self, addr: u8) -> u8 {
        self.cpu.sfr(addr, &self.ctx)
    }

    /// Write an SFR.
    pub fn set_sfr(&mut self, addr: u8, value: u8) {
        self.cpu.sfr_set(addr, value, &mut self.ctx);
    }

    /// Everything transmitted on the serial port.
    pub fn console(&self) -> &[u8] {
        &self.ctx.0.uart.console
    }

    /// The console as text.
    pub fn console_string(&self) -> String {
        self.ctx.0.uart.console_string()
    }

    /// Queue bytes as if they arrived on RXD.
    pub fn feed_uart(&mut self, bytes: &[u8]) {
        self.ctx.0.uart.feed_bytes(bytes);
    }

    /// A snapshot of the visible machine state.
    pub fn snapshot(&self) -> Snapshot {
        Snapshot {
            pc: self.pc(),
            a: self.cpu.a(),
            b: self.cpu.b(),
            dptr: self.cpu.dptr(),
            sp: self.cpu.sp(),
            psw: self.cpu.psw_value(),
            cycles: self.cycles,
            instructions: self.instructions,
            tx_bytes: self.ctx.0.uart.tx_count,
            timer0: self.ctx.0.timers.timer0.value(),
            timer1: self.ctx.0.timers.timer1.value(),
            tcon: self.ctx.0.timers.tcon,
        }
    }

    /// Decode the instruction at `pc`, without executing it.
    pub fn decode_at_pc(&self) -> Result<Instruction, mcs51_isa::DecodeError> {
        let pc = self.pc();
        let len = mcs51_isa::instruction_length(self.ctx.2.read_at(pc)) as usize;
        let mut bytes = Vec::with_capacity(len);
        for i in 0..len {
            bytes.push(self.ctx.2.read_at(pc.wrapping_add(i as u16)));
        }
        decode(pc, &bytes)
    }

    /// Execute one instruction. Returns `false` once the machine has halted.
    pub fn step(&mut self) -> bool {
        if self.ctx.0.power_down {
            return false;
        }
        if self.ctx.0.idle {
            // Sleeping: peripherals advance one machine cycle at a time and an
            // enabled interrupt wakes the core.
            self.ctx.0.tick(1);
            self.cycles += 1;
            if self.ctx.0.interrupt_pending(self.cpu.ie()) {
                self.ctx.0.idle = false;
            }
            return true;
        }

        let pc = self.pc();
        let cycles = self
            .decode_at_pc()
            .map(|ins| ins.cycles().max(1) as u32)
            .unwrap_or(1);

        if self.trace_limit > 0 {
            if let Ok(instruction) = self.decode_at_pc() {
                if self.trace.len() >= self.trace_limit {
                    self.trace.pop_front();
                }
                self.trace.push_back(TraceEntry {
                    cycles: self.cycles,
                    pc,
                    instruction,
                    a: self.cpu.a(),
                });
            }
        }

        let alive = self.cpu.step(&mut self.ctx);

        self.cycles += cycles as u64;
        self.instructions += 1;
        self.ctx.0.tick(cycles);
        // The "interrupt in progress" latch is cleared by RETI, inside the
        // emulator, exactly as the hardware does. Clearing it here would let the
        // same interrupt be taken again before the handler ran a single
        // instruction.

        if !alive && self.halt_on_self_jump {
            return false;
        }
        true
    }

    /// Run until the machine halts or `max_instructions` have retired.
    pub fn run(&mut self, max_instructions: u64) -> StopReason {
        for _ in 0..max_instructions {
            if self.ctx.0.power_down {
                return StopReason::PowerDown;
            }
            if !self.step() {
                return StopReason::Halted;
            }
        }
        StopReason::StepLimit
    }

    /// Run until `predicate` returns true, or the budget runs out.
    pub fn run_until(
        &mut self,
        mut predicate: impl FnMut(&Machine) -> bool,
        max_instructions: u64,
    ) -> StopReason {
        if predicate(self) {
            return StopReason::Predicate;
        }
        for _ in 0..max_instructions {
            if !self.step() {
                return StopReason::Halted;
            }
            if predicate(self) {
                return StopReason::Predicate;
            }
        }
        StopReason::StepLimit
    }

    /// Run until the console contains `needle`.
    pub fn run_until_console(&mut self, needle: &[u8], max_instructions: u64) -> StopReason {
        let needle = needle.to_vec();
        self.run_until(
            |machine| {
                let console = machine.console();
                console.len() >= needle.len()
                    && console[console.len() - needle.len()..] == needle[..]
            },
            max_instructions,
        )
    }

    /// Run until at least `cycles` machine cycles have retired.
    pub fn run_cycles(&mut self, cycles: u64) -> StopReason {
        let target = self.cycles + cycles;
        self.run_until(|machine| machine.cycles >= target, u64::MAX)
    }

    /// Request an interrupt, as the hardware would when a flag becomes set.
    pub fn raise_interrupt(&mut self, interrupt: i8051::Interrupt) -> bool {
        self.cpu.interrupt(interrupt)
    }

    /// A single line describing the next instruction, for logging.
    pub fn disassemble_next(&self) -> String {
        match self.decode_at_pc() {
            Ok(instruction) => format!("{:04X}: {}", self.pc(), instruction),
            Err(error) => format!("{:04X}: <{error}>", self.pc()),
        }
    }

    /// A trace rendering: `cycles  pc  instruction`.
    pub fn render_trace(&self) -> String {
        self.trace
            .iter()
            .map(|entry| {
                format!(
                    "{:>10}  {:04X}  {}",
                    entry.cycles, entry.pc, entry.instruction
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}

/// Extra accessors the `i8051` CPU type does not expose directly.
trait PswValue {
    fn psw_value(&self) -> u8;
}

impl PswValue for Cpu {
    fn psw_value(&self) -> u8 {
        use i8051::Flag;
        let mut value = 0u8;
        for flag in Flag::all() {
            if self.psw(flag) {
                value |= 1 << (flag as u8);
            }
        }
        value
    }
}

/// Convenience: read the accumulator from a machine.
pub fn accumulator(machine: &Machine) -> u8 {
    machine.cpu.a()
}

#[cfg(test)]
mod tests {
    use super::*;
    use mcs51_isa::Mnemonic;

    #[test]
    fn runs_a_hand_assembled_program() {
        // MOV A,#0x42 ; MOV SBUF,A ; SJMP $
        let code = vec![0x74, 0x42, 0xF5, 0x99, 0x80, 0xFE];
        let mut machine = Machine::with_uart_delay(code, 0);
        assert_eq!(machine.run(100), StopReason::Halted);
        assert_eq!(machine.cpu().a(), 0x42);
        assert_eq!(machine.console(), b"B");
        assert_eq!(machine.instructions, 3);
        assert_eq!(machine.cycles, 1 + 1 + 2);
    }

    #[test]
    fn timer0_mode2_overflows() {
        // TMOD = 0x02 (timer 0, mode 2, timer)
        // TH0 = TL0 = 0xF0  -> overflow every 16 machine cycles
        // TCON = 0x10 (TR0 on)
        // loop: SJMP loop
        let code = vec![
            0x75, 0x89, 0x02, // MOV TMOD,#0x02
            0x75, 0x8C, 0xF0, // MOV TH0,#0xF0
            0x75, 0x8A, 0xF0, // MOV TL0,#0xF0
            0x75, 0x88, 0x10, // MOV TCON,#0x10
            0x80, 0xFE, // SJMP $
        ];
        let mut machine = Machine::new(code);
        machine.halt_on_self_jump = false;
        machine.run_cycles(200);
        assert!(
            machine.peripherals().timers.timer0.overflows >= 5,
            "timer should have wrapped: {}",
            machine.peripherals().timers.timer0.overflows
        );
        assert!(machine.peripherals().timers.overflow_flag(Which::Timer0));
    }

    #[test]
    fn trace_records_recent_instructions() {
        let code = vec![0x00, 0x00, 0x04, 0x80, 0xFE]; // NOP NOP INC A SJMP $
        let mut machine = Machine::new(code);
        machine.trace_limit = 10;
        machine.run(100);
        assert_eq!(machine.trace.len(), 4);
        assert_eq!(machine.trace[0].instruction.mnemonic, Mnemonic::Nop);
        assert_eq!(machine.trace[2].instruction.mnemonic, Mnemonic::Inc);
        assert!(machine.render_trace().contains("INC A"));
    }

    /// Every SFR must be readable back, and the port latches must not shadow
    /// their neighbours. `PCON` aliasing the `P0` latch once put the CPU
    /// straight into power-down.
    #[test]
    fn sfr_writes_read_back_and_ports_do_not_alias() {
        let mut machine = Machine::new(vec![0x80, 0xFE]); // SJMP $
        machine.peripherals_mut().poke(sfr::SFR_P0, 0xAA);
        machine.peripherals_mut().poke(sfr::SFR_P1, 0x55);
        assert_eq!(machine.peripherals().peek(sfr::SFR_P0), 0xAA);
        assert_eq!(machine.peripherals().peek(sfr::SFR_P1), 0x55);

        machine.peripherals_mut().poke(sfr::SFR_PCON, 0x01);
        assert_eq!(machine.peripherals().peek(sfr::SFR_PCON), 0x01);
        assert!(machine.peripherals().idle);
        assert!(!machine.peripherals().power_down);
        // and the port next door is untouched
        assert_eq!(machine.peripherals().peek(sfr::SFR_P0), 0xAA);
    }

    #[test]
    fn port_writes_can_be_logged() {
        // MOV P1,#0x55 ; SJMP $
        let code = vec![0x75, 0x90, 0x55, 0x80, 0xFE];
        let mut machine = Machine::new(code);
        machine.peripherals_mut().log_ports = true;
        machine.run(10);
        assert_eq!(machine.peripherals().port_log, vec![(0x90, 0x55)]);
        assert_eq!(machine.peripherals().latch(1), 0x55);
    }

    #[test]
    fn idle_mode_wakes_on_timer_interrupt() {
        // A real vector table: reset jumps to start, timer 0 has an ISR.
        let mut code = vec![0xFFu8; 0x60];
        // 0x0000: LJMP start
        code[0..3].copy_from_slice(&[0x02, 0x00, 0x30]);
        // 0x000B: RETI
        code[0x0B] = 0x32;
        // 0x0030: set up timer 0 in mode 2 counting from 0, enable ET0|EA,
        //         start it, then idle.
        let program = [
            0x75, 0x89, 0x02, // MOV TMOD,#0x02
            0x75, 0x8C, 0x00, // MOV TH0,#0
            0x75, 0x8A, 0x00, // MOV TL0,#0
            0x75, 0x88, 0x10, // MOV TCON,#0x10  (TR0)
            0x75, 0xA8, 0x82, // MOV IE,#0x82    (EA|ET0)
            0x75, 0x87, 0x01, // MOV PCON,#0x01  (IDL)
            0x80, 0xFE, // SJMP $
        ];
        code[0x30..0x30 + program.len()].copy_from_slice(&program);

        let mut machine = Machine::new(code);
        machine.run_cycles(600);
        assert!(
            !machine.peripherals().idle,
            "a timer interrupt should have woken it"
        );
        assert!(
            machine.peripherals().timers.timer0.overflows >= 1,
            "timer 0 should have overflowed"
        );
    }
}
