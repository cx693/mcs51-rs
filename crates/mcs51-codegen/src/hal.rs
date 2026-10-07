//! 高层外设封装：说你要什么，而不是逐条配 SFR。
//!
//! 这一层存在的理由：8051 的裸机代码里，真正烦人的不是指令本身，而是**配寄存器
//! 的那一堆琐事** —— `TMOD` 的高 4 位归 Timer 1、低 4 位归 Timer 0，改一个必须
//! 读-改-写；波特率要按 `fosc / (384 * baud)` 反推 `TH1`；`PCON` 不是可位寻址的，
//! 只能整字节改。这些算术**在主机上算一次就够了**，没必要让每个应用手写一遍。
//!
//! 所以这里的对象都是「主机侧的描述 + 生成目标机代码的方法」：
//!
//! ```no_run
//! # use mcs51_chips::stc89c5xrc::*;
//! # use mcs51_codegen::prelude::*;
//! let cpu_hz = 11_059_200;
//!
//! // 引脚：带上极性，读代码的人不用再去想"低电平是亮还是灭"
//! let led = Gpio::active_low(P35);
//!
//! // 定时器：周期直接说微秒，重载值在主机上算好
//! let tick = Timer::timer0().micros(cpu_hz, 200);
//!
//! // 串口：波特率直接说，TH1 在主机上算好
//! let uart = Uart::new(cpu_hz, 9600);
//!
//! let mut app = App::new(Board::default());
//! app.main(move |t| {
//!     uart.begin(t);
//!     tick.begin(t);
//!     t.forever(|t| {
//!         tick.clear_flag(t);
//!         tick.reload(t);
//!         led.toggle(t);
//!     });
//! });
//! ```

use mcs51_chips::reg52::{PCON, SBUF, SCON, TH0, TH1, TL0, TL1, TMOD};
use mcs51_chips::{Bit, Sfr};

use crate::App;
use crate::emit::{Reg, Routine, Task};

// ===========================================================================
// GPIO
// ===========================================================================

/// 一个引脚，带**极性**。
///
/// 8051 的 IO 是准双向口：写 1 就是弱上拉（高，可以当输入读），写 0 是强下拉。
/// 没有方向寄存器要配 —— 所以"设置步骤多"的观感其实来自**极性**：
/// `SETB P3_5` 到底是点亮还是熄灭，取决于 LED 接在 VCC 还是 GND 那边。
///
/// [`Gpio`] 把这件事说清楚，代码里就只剩 `on()` / `off()`。
///
/// ```
/// # use mcs51_chips::stc89c5xrc::*;
/// # use mcs51_codegen::prelude::*;
/// let led = Gpio::active_low(P35);     // LED 接 VCC → 引脚拉低才亮
/// let mut app = App::new(Board::default());
/// app.main(move |t| {
///     led.on(t);                        // CLR P3_5
///     led.off(t);                       // SETB P3_5
///     led.toggle(t);                    // CPL P3_5
/// });
/// let asm = app.finish().unwrap().assembly;
/// assert!(asm.contains("CLR   P35"));
/// assert!(asm.contains("SETB  P35"));
/// assert!(asm.contains("CPL   P35"));
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Gpio {
    pin: Bit,
    active_low: bool,
}

impl Gpio {
    /// 低电平有效（LED 阳极接 VCC，阴极经电阻接引脚 —— 最常见的接法）。
    pub const fn active_low(pin: Bit) -> Self {
        Self {
            pin,
            active_low: true,
        }
    }

    /// 高电平有效（引脚经电阻接 LED 阳极，阴极接 GND）。
    pub const fn active_high(pin: Bit) -> Self {
        Self {
            pin,
            active_low: false,
        }
    }

    /// 底层引脚，需要时仍可直接做位操作。
    pub const fn pin(self) -> Bit {
        self.pin
    }

    /// 引脚名，用于注释。
    pub const fn name(self) -> &'static str {
        self.pin.name
    }

    /// 点亮（按极性选 `CLR` 还是 `SETB`）。
    pub fn on(&self, t: &mut Task) {
        if self.active_low {
            t.clr(self.pin);
        } else {
            t.setb(self.pin);
        }
    }

    /// 熄灭。
    pub fn off(&self, t: &mut Task) {
        if self.active_low {
            t.setb(self.pin);
        } else {
            t.clr(self.pin);
        }
    }

    /// 翻转。`CPL` 只认电平，和极性无关。
    pub fn toggle(&self, t: &mut Task) {
        t.cpl(self.pin);
    }

    /// 交给上拉（写 1）。准双向口下引脚变成弱上拉，可以当输入读。
    pub fn release(&self, t: &mut Task) {
        t.setb(self.pin);
    }

    /// 按当前电平跳转。**这是目标机运行期的分支**，不是 Rust 的 `if`。
    pub fn branch_if_on(&self, t: &mut Task, label: crate::emit::Label) {
        if self.active_low {
            t.jnb(self.pin, label);
        } else {
            t.jb(self.pin, label);
        }
    }

    /// 按当前电平跳转（熄灭时跳）。
    pub fn branch_if_off(&self, t: &mut Task, label: crate::emit::Label) {
        if self.active_low {
            t.jb(self.pin, label);
        } else {
            t.jnb(self.pin, label);
        }
    }

    /// 把引脚电平读进进位位 `C`。
    pub fn read_into_carry(&self, t: &mut Task) {
        t.mov_c_pin(self.pin);
    }
}

// ===========================================================================
// 定时器
// ===========================================================================

/// 两个定时器。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimerId {
    /// Timer 0：`TH0/TL0`、`TR0/TF0`，占用 `TMOD` 的**低** 4 位。
    T0,
    /// Timer 1：`TH1/TL1`、`TR1/TF1`，占用 `TMOD` 的**高** 4 位。
    /// 串口波特率通常占着它（模式 2）。
    T1,
}

/// 一个定时器的配置（模式 1：16 位定时）。
///
/// 重载值在**主机上**算好 —— 目标机只做"重装两个字节"。
///
/// ```
/// # use mcs51_codegen::prelude::*;
/// let tick = Timer::timer0().micros(11_059_200, 200);
/// // 200us @ 11.0592MHz，一个机器周期 12 个时钟
/// assert_eq!(tick.counted_cycles(), 184);
/// assert_eq!(tick.reload_value(), 0xFF48);   // 65536 - 184
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Timer {
    id: TimerId,
    counted: u32,
    /// 请求的周期超过了 16 位定时器的量程。
    ///
    /// **不静默钳位** —— 悄悄把 500ms 改成 71ms 只会让人对着示波器怀疑人生。
    /// 记下来，等 `begin()` 报成构建错误，或者改用 [`Ticker`]。
    overflowed: bool,
}

/// 16 位定时器一次最多能数多少个机器周期。
pub const MAX_COUNTED: u32 = 65_536;

impl Timer {
    /// Timer 0。
    pub const fn timer0() -> Self {
        Self::new(TimerId::T0)
    }

    /// Timer 1。
    pub const fn timer1() -> Self {
        Self::new(TimerId::T1)
    }

    /// 指定哪一个。
    pub const fn new(id: TimerId) -> Self {
        Self {
            id,
            counted: 1,
            overflowed: false,
        }
    }

    /// 直接指定要数的**机器周期**个数（1..=65536）。
    ///
    /// 超出量程不会报错在这里，而是记下来由 `begin()` 报出 —— 因为
    /// `Timer` 是常量上下文里构造的，没有地方打印诊断。
    pub const fn cycles(mut self, cycles: u32) -> Self {
        if cycles == 0 {
            self.counted = 1;
        } else if cycles > MAX_COUNTED {
            self.counted = MAX_COUNTED;
            self.overflowed = true;
        } else {
            self.counted = cycles;
        }
        self
    }

    /// 指定周期，单位微秒；主机按晶振换算成机器周期。
    pub const fn micros(self, cpu_hz: u32, us: u32) -> Self {
        let cycles = ((us as u64 * cpu_hz as u64) / 12_000_000) as u32;
        self.cycles(cycles)
    }

    /// 指定周期，单位毫秒。
    ///
    /// **注意**：16 位定时器在 11.0592MHz 下最多数约 71ms。更长的周期用
    /// [`Timer::every_ms`]，它会自动把溢出次数算进去。
    pub const fn millis(self, cpu_hz: u32, ms: u32) -> Self {
        self.micros(cpu_hz, ms * 1000)
    }

    /// 每 `ms` 毫秒一次的节拍器 —— 周期超过定时器量程也能用。
    pub const fn every_ms(self, cpu_hz: u32, ms: u32) -> Ticker {
        Ticker::build(self.id, cpu_hz, ms as u64 * 1000, POLLED_OVERHEAD)
    }

    /// 每 `us` 微秒一次的节拍器。
    pub const fn every_us(self, cpu_hz: u32, us: u32) -> Ticker {
        Ticker::build(self.id, cpu_hz, us as u64, POLLED_OVERHEAD)
    }

    /// 从要数的周期里扣掉一段固定开销。
    ///
    /// 定时器在软件**发现溢出之前就已经继续计数了**，所以实际周期是
    /// `重载值 + 开销`。开销是十几条指令的量级：
    ///
    /// * 主循环里"等 `TF0` → 清标志 → 重装"：约 16 个机器周期
    /// * 中断服务程序入口 + 保存 5 个寄存器 + 重载：约 14 个
    ///
    /// 周期长（几百微秒以上）时这点开销无所谓；周期很短时明显偏慢。
    pub const fn compensate(mut self, overhead_cycles: u32) -> Self {
        let counted = self.counted.saturating_sub(overhead_cycles);
        self.counted = if counted == 0 { 1 } else { counted };
        self
    }

    /// 要数的机器周期数。
    pub const fn counted_cycles(&self) -> u32 {
        self.counted
    }

    /// `TH/TL` 的重载值（主机侧的计算结果）。
    pub const fn reload_value(&self) -> u16 {
        (65_536u32 - self.counted) as u16
    }

    /// 定时器实际会达到的频率（只算计数部分，不含软件开销）。
    pub fn actual_hz(&self, cpu_hz: u32) -> f64 {
        (cpu_hz as f64 / 12.0) / self.counted as f64
    }

    /// 溢出标志（`TF0` / `TF1`）。
    pub const fn overflow_flag(&self) -> Bit {
        match self.id {
            TimerId::T0 => mcs51_chips::reg52::TF0,
            TimerId::T1 => mcs51_chips::reg52::TF1,
        }
    }

    /// 启动位（`TR0` / `TR1`）。
    pub const fn run_bit(&self) -> Bit {
        match self.id {
            TimerId::T0 => mcs51_chips::reg52::TR0,
            TimerId::T1 => mcs51_chips::reg52::TR1,
        }
    }

    /// 高字节寄存器。
    pub const fn high_byte(&self) -> Sfr {
        match self.id {
            TimerId::T0 => TH0,
            TimerId::T1 => TH1,
        }
    }

    /// 低字节寄存器。
    pub const fn low_byte(&self) -> Sfr {
        match self.id {
            TimerId::T0 => TL0,
            TimerId::T1 => TL1,
        }
    }

    /// `TMOD` 里属于这个定时器的模式位：模式 1 = 16 位定时器、`GATE=0`、`C/T=0`。
    const fn mode1_nibble(&self) -> u8 {
        match self.id {
            TimerId::T0 => 0x01,
            TimerId::T1 => 0x10,
        }
    }

    /// `TMOD` 里属于**另一个**定时器的位，读-改-写时要保留。
    const fn other_mask(&self) -> u8 {
        match self.id {
            TimerId::T0 => 0xF0,
            TimerId::T1 => 0x0F,
        }
    }

    /// 配好并启动：读-改-写 `TMOD`，装初值，`SETB TRx`。
    ///
    /// 周期超过 16 位量程的话这里是**构建错误**，而不是悄悄给你一个错的周期。
    pub fn begin(&self, t: &mut Task) {
        if self.overflowed {
            t.fail(format!(
                "Timer 周期要 {} 个机器周期，超过 16 位定时器的量程 {}。\
                 11.0592MHz 下一个机器周期约 1.085us，量程约 71ms。\
                 更长的周期用 `Timer::timer0().every_ms(cpu_hz, ms)`，\
                 它会自动数溢出次数。",
                self.counted, MAX_COUNTED
            ));
            return;
        }
        t.comment(&format!(
            "{}：模式 1（16 位定时），数 {} 个机器周期",
            match self.id {
                TimerId::T0 => "Timer 0",
                TimerId::T1 => "Timer 1",
            },
            self.counted,
        ));
        t.mov_a_direct(TMOD);
        t.anl_a_imm(self.other_mask());
        t.orl_a_imm(self.mode1_nibble());
        t.mov_direct_a(TMOD);
        self.reload(t);
        t.setb(self.run_bit());
    }

    /// 重装初值。模式 1 不会自动重装，每次溢出后都要自己来一遍。
    pub fn reload(&self, t: &mut Task) {
        let value = self.reload_value();
        t.mov_direct_imm(self.high_byte(), (value >> 8) as u8);
        t.mov_direct_imm(self.low_byte(), value as u8);
    }

    /// 启动。
    pub fn start(&self, t: &mut Task) {
        t.setb(self.run_bit());
    }

    /// 停止。
    pub fn stop(&self, t: &mut Task) {
        t.clr(self.run_bit());
    }

    /// 清溢出标志（模式 1 下必须手动清）。
    pub fn clear_flag(&self, t: &mut Task) {
        t.clr(self.overflow_flag());
    }

    /// **等一次溢出，然后清标志、重装** —— 三件事一次做完。
    ///
    /// 典型用法：`t.forever(|t| { tick.wait(t); ... })`。
    pub fn wait(&self, t: &mut Task) {
        let spin = t.new_label("wait");
        t.label(spin);
        t.jnb(self.overflow_flag(), spin);
        self.clear_flag(t);
        self.reload(t);
    }
}

// ===========================================================================
// 节拍器：周期超过定时器量程时自动数溢出次数
// ===========================================================================

/// "在循环里等溢出、清标志、重装"这一段的固定开销，机器周期。
///
/// 一个 `JNB`（2 周期）+ 一个 `CLR`（1 周期）+ 两条 `MOV direct,#imm`（4 周期）
/// 加上分支本身，量出来是 14~18 之间，取 16。
pub const POLLED_OVERHEAD: u32 = 16;

/// 一个"每 N 毫秒来一次"的节拍器。
///
/// 16 位定时器数不下很长的周期（11.0592MHz 下约 71ms 封顶），所以
/// 500ms 这种要拆成"溢出 8 次"。手算这件事很容易错 —— 错的方式还是
/// **静默给出错误周期**，所以封在主机侧：
///
/// ```
/// # use mcs51_codegen::prelude::*;
/// // 500ms @ 11.0592MHz：一次装不下，拆成 8 次溢出
/// let tick = Timer::timer0().every_ms(11_059_200, 500);
/// assert_eq!(tick.overflows(), 8);
/// // 460800 均分给 8 次溢出 = 每次 57600，再扣掉 16 个周期的软件开销
/// assert_eq!(tick.timer().counted_cycles(), 57_584);
/// // 于是总时长正好是 500ms
/// assert_eq!(tick.actual_us(11_059_200), 500_000);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ticker {
    timer: Timer,
    overflows: u16,
}

impl Ticker {
    /// 算出"分几次溢出、每次数多少周期"。
    const fn build(id: TimerId, cpu_hz: u32, us: u64, overhead: u32) -> Self {
        // 一个机器周期 = 12 个时钟。
        let wanted = (us * cpu_hz as u64) / 12_000_000;
        let wanted = if wanted == 0 { 1 } else { wanted };

        // 每次溢出最多能数这么多（留出开销的位置）。
        let per_overflow_max = (MAX_COUNTED - overhead) as u64;
        let mut overflows = wanted.div_ceil(per_overflow_max);
        if overflows == 0 {
            overflows = 1;
        }
        if overflows > u16::MAX as u64 {
            overflows = u16::MAX as u64;
        }

        // 把总周期**均分**给每次溢出，这样总时长才准。
        // （不均分的话 8 × 65520 = 524160，比要的 460800 多 14%。）
        let base = (wanted / overflows) as u32;
        let floor = overhead + 1;
        let counted = if base < floor { floor } else { base } - overhead;

        Self {
            timer: Timer::new(id).cycles(counted),
            overflows: overflows as u16,
        }
    }

    /// 底层定时器配置。
    pub const fn timer(&self) -> Timer {
        self.timer
    }

    /// 一次节拍要等几次溢出。
    pub const fn overflows(&self) -> u16 {
        self.overflows
    }

    /// 实际周期（微秒）。
    pub const fn actual_us(&self, cpu_hz: u32) -> u64 {
        let cycles = (self.timer.counted_cycles() + POLLED_OVERHEAD) as u64 * self.overflows as u64;
        cycles * 12_000_000 / cpu_hz as u64
    }

    /// 配好并启动。
    pub fn begin(&self, t: &mut Task) {
        self.timer.begin(t);
    }

    /// 等一次节拍（内部把 `overflows` 次等待展开成顺序代码，不需要额外 RAM）。
    pub fn wait(&self, t: &mut Task) {
        if self.overflows <= 1 {
            self.timer.wait(t);
            return;
        }
        if self.overflows > 16 {
            t.warn(format!(
                "Ticker 要展开 {} 次溢出等待（约 {} 字节代码）。\
                 周期很长时建议改用 RAM 里的计数器 + 一次定长等待。",
                self.overflows,
                self.overflows as u32 * 8
            ));
        }
        t.comment(&format!(
            "等 {} 次定时器溢出（展开成顺序代码，不用额外变量）",
            self.overflows
        ));
        for _ in 0..self.overflows {
            self.timer.wait(t);
        }
    }

    /// 停止定时器。
    pub fn stop(&self, t: &mut Task) {
        self.timer.stop(t);
    }
}

// ===========================================================================
// 串口
// ===========================================================================

/// 串口（模式 1：8 位 UART，Timer 1 供波特率）。
///
/// `TH1` 在主机上算好：
/// `baud = fosc / (divider * (256 - TH1))`，`divider` 是 384，`SMOD=1` 时 192。
///
/// ```
/// # use mcs51_codegen::prelude::*;
/// // 11.0592MHz 让 9600 精确整除
/// let uart = Uart::new(11_059_200, 9600);
/// assert_eq!(uart.reload(), 0xFD);
/// assert_eq!(uart.actual_baud(), 9600);
///
/// // 12MHz 做不到，会报出误差
/// let off = Uart::new(12_000_000, 9600);
/// assert!(off.baud_error() > 0.08);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Uart {
    cpu_hz: u32,
    baud: u32,
    smod: bool,
}

impl Uart {
    /// 指定晶振和波特率。
    pub const fn new(cpu_hz: u32, baud: u32) -> Self {
        Self {
            cpu_hz,
            baud: if baud == 0 { 1 } else { baud },
            smod: false,
        }
    }

    /// 打开 `PCON.SMOD`（波特率加倍）。在 12MHz 这类晶振上能救回一些除不尽的档位。
    pub const fn smod(mut self, on: bool) -> Self {
        self.smod = on;
        self
    }

    /// 波特率发生器的分频系数。
    const fn divider(&self) -> u32 {
        if self.smod { 192 } else { 384 }
    }

    /// `TH1` 的值（四舍五入，不是截断 —— 截断会让高频档位明显偏慢）。
    pub const fn reload(&self) -> u8 {
        let denominator = self.divider() as u64 * self.baud as u64;
        let divisor = (self.cpu_hz as u64 + denominator / 2) / denominator;
        let divisor = if divisor == 0 { 1 } else { divisor };
        (256u64.saturating_sub(divisor)) as u8
    }

    /// 实际能达到的波特率。
    pub const fn actual_baud(&self) -> u32 {
        let denominator = self.divider() as u64 * (256 - self.reload() as u64);
        if denominator == 0 {
            return 0;
        }
        (self.cpu_hz as u64 / denominator) as u32
    }

    /// 实际波特率相对目标值的误差（0.085 = 8.5%）。
    pub fn baud_error(&self) -> f64 {
        let actual = self.actual_baud() as f64;
        (actual - self.baud as f64).abs() / self.baud as f64
    }

    /// 配好并使能：Timer 1 模式 2 供波特率，`SCON` 模式 1 开接收。
    pub fn begin(&self, t: &mut Task) {
        t.comment(&format!(
            "串口：{} baud（实际 {}，误差 {:.2}%），8 位 UART，Timer 1 供时钟",
            self.baud,
            self.actual_baud(),
            self.baud_error() * 100.0
        ));
        // Timer 1 模式 2（8 位自动重装），保留 TMOD 里 Timer 0 的那一半。
        t.mov_a_direct(TMOD);
        t.anl_a_imm(0x0F);
        t.orl_a_imm(0x20);
        t.mov_direct_a(TMOD);

        let reload = self.reload();
        t.mov_direct_imm(TH1, reload);
        t.mov_direct_imm(TL1, reload);
        t.mov_direct_imm(SCON, 0x50); // 模式 1（8 位 UART）+ REN

        // PCON 不是可位寻址的，SMOD 只能整字节读-改-写。
        t.mov_a_direct(PCON);
        if self.smod {
            t.orl_a_imm(0x80);
        } else {
            t.anl_a_imm(0x7F);
        }
        t.mov_direct_a(PCON);

        t.setb(mcs51_chips::reg52::TR1);
    }

    /// 生成发送一个字节的目标机子程序（入参 `A`），返回句柄。
    ///
    /// 轮询 `TI`：8051 没有发送 FIFO，写 `SBUF` 之后必须等硬件把这一帧移完。
    pub fn install_tx(&self, app: &mut App) -> Routine {
        app.routine("uart_tx", |t| {
            t.comment("发送 A 里的一个字节");
            t.mov_direct_a(SBUF);
            let wait = t.new_label("tx_wait");
            t.label(wait);
            t.jnb(mcs51_chips::reg52::TI, wait);
            t.clr(mcs51_chips::reg52::TI);
        })
    }

    /// 生成发送 NUL 结尾字符串的子程序（入参 `DPTR`，串在 CODE 段），返回句柄。
    ///
    /// 它调用 [`Uart::install_tx`] 生成的子程序，所以要先装那个。
    pub fn install_puts(&self, app: &mut App, tx: &Routine) -> Routine {
        let tx = tx.clone();
        app.routine("uart_puts", move |t| {
            t.comment("发送 DPTR 指向的 NUL 结尾字符串");
            let again = t.new_label("puts_again");
            let done = t.new_label("puts_done");
            t.label(again);
            t.clr_a();
            t.movc_a_dptr(); // 从 CODE 段取一个字节
            t.jz(done);
            t.call_routine(&tx);
            t.inc_dptr();
            t.sjmp(again);
            t.label(done);
        })
    }

    /// 发送一个字符串。串会进 CODE 段，调用点只有 `MOV DPTR` + `LCALL` 几条指令。
    pub fn puts(&self, t: &mut Task, puts: &Routine, text: &str) {
        let label = t.string_label(text);
        t.mov_dptr_symbol(&label);
        t.call_routine(puts);
    }

    /// 把寄存器里的字节发出去（通过 [`Uart::install_tx`] 的子程序）。
    pub fn putc(&self, t: &mut Task, tx: &Routine, reg: Reg) {
        t.mov_a_reg(reg);
        t.call_routine(tx);
    }

    /// 有字节收到就跳到 `label`（`JB RI, label`）。
    ///
    /// 8051 的接收也是轮询的：没有数据就继续往下走。
    pub fn branch_if_rx_ready(&self, t: &mut Task, label: crate::emit::Label) {
        t.jb(mcs51_chips::reg52::RI, label);
    }

    /// 读取收到的字节到 `A`，并清 `RI`。
    ///
    /// **顺序不能反**：先读 `SBUF` 再清 `RI`。反过来的话，清 `RI` 与读 `SBUF`
    /// 之间到的新字节会被硬件丢弃（这一条在真实硬件上验证过）。
    pub fn getc(&self, t: &mut Task) {
        t.mov_a_direct(SBUF);
        t.clr(mcs51_chips::reg52::RI);
    }
}

// ===========================================================================
// 控制台：串口打印一次装好
// ===========================================================================

/// 串口控制台。
///
/// 裸机串口要自己写四个子程序（发字节、发字符串、十六进制、十进制），
/// 每个应用抄一遍没意义。这里一次装好，之后 `print` / `println` 一行搞定：
///
/// ```no_run
/// # use mcs51_chips::stc89c5xrc::*;
/// # use mcs51_codegen::prelude::*;
/// let mut app = App::new(Board::default());
/// let console = Console::install(&mut app, 11_059_200, 9600);
/// app.main(move |t| {
///     console.begin(t);                 // 串口全配好（Timer 1 + TH1 + SCON + PCON）
///     console.println(t, "hello");
///     t.mov_a_imm(42);
///     console.print_dec8(t);            // 42 → "42"
///     console.println(t, "");
/// });
/// ```
///
/// 生成的子程序在目标机上各只有一份：
///
/// | 子程序 | 入参 | 作用 |
/// |---|---|---|
/// | `__uart_tx` | `A` | 发一个字节（轮询 `TI`） |
/// | `__uart_puts` | `DPTR` | 发 CODE 段里 NUL 结尾的串 |
/// | `__uart_hex8` | `A` | 两位十六进制 |
/// | `__uart_dec8` | `A` | 十进制（无前导零） |
#[derive(Debug, Clone)]
pub struct Console {
    uart: Uart,
    tx: Routine,
    puts: Routine,
    /// 十六进制打印子程序，用 [`Console::with_numbers`] 才生成。
    hex8: Option<Routine>,
    /// 十进制打印子程序，同上。
    dec8: Option<Routine>,
}

impl Console {
    /// 装上最基本的两个子程序：发一个字节、发一段字符串（约 20 字节代码）。
    ///
    /// 需要打印数字再接 [`Console::with_hex`]（约 25 字节）或
    /// [`Console::with_dec`]（约 45 字节）—— 用不到的子程序不该占 Flash。
    ///
    /// ```no_run
    /// # use mcs51_chips::stc89c5xrc::*;
    /// # use mcs51_codegen::prelude::*;
    /// let mut app = App::new(Board::default());
    /// let console = Console::install(&mut app, 11_059_200, 9600)
    ///     .with_hex(&mut app);              // 需要打十六进制才加这一句
    /// # let _ = console;
    /// ```
    pub fn install(app: &mut App, cpu_hz: u32, baud: u32) -> Self {
        let uart = Uart::new(cpu_hz, baud);
        let tx = uart.install_tx(app);
        let puts = uart.install_puts(app, &tx);
        Self {
            uart,
            tx,
            puts,
            hex8: None,
            dec8: None,
        }
    }

    /// 再装上十六进制打印（约 25 字节）。
    pub fn with_hex(mut self, app: &mut App) -> Self {
        let tx = self.tx.clone();
        self.hex8 = Some(Self::install_hex8(app, &tx));
        self
    }

    /// 再装上十进制打印（约 45 字节）。
    pub fn with_dec(mut self, app: &mut App) -> Self {
        let tx = self.tx.clone();
        self.dec8 = Some(Self::install_dec8(app, tx));
        self
    }

    /// 十六进制 + 十进制都装上。
    pub fn with_numbers(self, app: &mut App) -> Self {
        self.with_hex(app).with_dec(app)
    }

    /// 生成"把 `A` 打成两位十六进制"的子程序。
    fn install_hex8(app: &mut App, tx: &Routine) -> Routine {
        let tx = tx.clone();

        // 一个十六进制位
        let tx_for_nibble = tx.clone();
        let nibble = app.routine("uart_hex_nibble", move |t| {
            t.anl_a_imm(0x0F);
            t.add_a_imm(b'0');
            // '9' 之后要跳过 7 个字符才到 'A'。
            let compare = t.new_label("hex_compare");
            let send = t.new_label("hex_send");
            t.cjne_a_imm(b':', compare);
            t.label(compare);
            t.jc(send);
            t.add_a_imm(7);
            t.label(send);
            t.call_routine(&tx_for_nibble);
        });

        app.routine("uart_hex8", move |t| {
            t.push_direct(0x07); // PUSH 0x07 压的就是 R7
            t.mov_reg_a(Reg::R7); // R7 ← A
            t.swap_a();
            t.call_routine(&nibble);
            t.mov_a_reg(Reg::R7); // A ← R7
            t.call_routine(&nibble);
            t.pop_direct(0x07);
        })
    }

    /// 生成"把 `A` 打成十进制（0..255，无前导零）"的子程序。
    fn install_dec8(app: &mut App, tx: Routine) -> Routine {
        // 十进制：先除 100 得百位，余数再除 10 得十位和个位。
        let tx_for_dec = tx;
        app.routine("uart_dec8", move |t| {
            // 这四条要自己保存，因为 DIV AB 会动 B，除法结果要用 R5/R6/R7。
            t.push_direct(0xF0); // B 在直接地址 0xF0
            t.push_direct(0x07);
            t.push_direct(0x06);
            t.push_direct(0x05);

            t.mov_b_imm(100);
            t.div_ab();
            t.mov_reg_a(Reg::R7); // R7 = 百位
            t.mov_a_b(); // A = 余数
            t.mov_b_imm(10);
            t.div_ab();
            t.mov_reg_a(Reg::R6); // R6 = 十位
            t.mov_reg_b(Reg::R5); // R5 = 个位

            let no_hundreds = t.new_label("dec_no_hundreds");
            let tens = t.new_label("dec_tens");
            let units = t.new_label("dec_units");

            t.mov_a_reg(Reg::R7); // 百位
            t.jz(no_hundreds);
            t.add_a_imm(b'0');
            t.call_routine(&tx_for_dec);
            t.label(no_hundreds);
            t.mov_a_reg(Reg::R7); // 百位非零 → 十位必须打
            t.jnz(tens);
            t.mov_a_reg(Reg::R6); // 否则十位非零才打
            t.jz(units);
            t.label(tens);
            t.mov_a_reg(Reg::R6);
            t.add_a_imm(b'0');
            t.call_routine(&tx_for_dec);
            t.label(units);
            t.mov_a_reg(Reg::R5); // 个位无论如何都要打
            t.add_a_imm(b'0');
            t.call_routine(&tx_for_dec);

            t.pop_direct(0x05);
            t.pop_direct(0x06);
            t.pop_direct(0x07);
            t.pop_direct(0xF0);
        })
    }

    /// 底层串口配置。
    pub fn uart(&self) -> &Uart {
        &self.uart
    }

    /// 配好并使能串口（Timer 1 供波特率，`SCON` 模式 1 开接收）。
    pub fn begin(&self, t: &mut Task) {
        self.uart.begin(t);
    }

    /// 发一段文本（不需要结尾的 `\0`，`string_label` 会补）。
    pub fn print(&self, t: &mut Task, text: &str) {
        self.uart.puts(t, &self.puts, text);
    }

    /// 发一段文本再换行。
    pub fn println(&self, t: &mut Task, text: &str) {
        self.uart.puts(t, &self.puts, text);
        self.uart.puts(t, &self.puts, "\r\n");
    }

    /// 只发一个换行（`\r\n`）。
    pub fn newline(&self, t: &mut Task) {
        self.uart.puts(t, &self.puts, "\r\n");
    }

    /// 把 `A` 发出去。
    pub fn putc(&self, t: &mut Task) {
        t.call_routine(&self.tx);
    }

    /// 把寄存器里的字节发出去。
    pub fn print_char(&self, t: &mut Task, reg: Reg) {
        self.uart.putc(t, &self.tx, reg);
    }

    /// 把 `A` 打成两位十六进制。需要先 [`Console::with_numbers`]。
    pub fn print_hex8(&self, t: &mut Task) {
        match &self.hex8 {
            Some(routine) => {
                t.call_routine(routine);
            }
            None => t.fail("print_hex8 需要先在 Console::install 后面接 .with_hex(&mut app)"),
        }
    }

    /// 把 `A` 打成十进制（0..255，无前导零）。需要先 [`Console::with_numbers`]。
    pub fn print_dec8(&self, t: &mut Task) {
        match &self.dec8 {
            Some(routine) => {
                t.call_routine(routine);
            }
            None => t.fail("print_dec8 需要先在 Console::install 后面接 .with_dec(&mut app)"),
        }
    }

    /// 有字节收到就跳到 `label`。
    pub fn branch_if_rx_ready(&self, t: &mut Task, label: crate::emit::Label) {
        self.uart.branch_if_rx_ready(t, label);
    }

    /// 读一个字节到 `A` 并清 `RI`。
    pub fn getc(&self, t: &mut Task) {
        self.uart.getc(t);
    }
}
