//! # mcs51-codegen — 用 Rust 编写 8051 固件
//!
//! 这个 crate 让**应用代码用 Rust 写**（`main.rs`），在**构建期**生成 8051 机器码，
//! 再交给本仓库自己的汇编器 / 链接器 / 打包器。汇编那条路完全保留。
//!
//! ## 为什么不是「Rust 直接编译到 8051」
//!
//! `rustc` 依赖 LLVM 后端，而 LLVM 没有 MCS-51 target：
//!
//! ```text
//! $ rustc --print target-list | grep -i 8051
//!   （无）
//! $ clang --print-targets | grep -i 8051
//!   （无）
//! ```
//!
//! 8 位平台里只有 `avr-none`、`msp430-none-elf` 有 LLVM 后端 —— 因为它们有人写了。
//! MCS-51 至今没有被合并进 LLVM 的后端，GCC 也没有。
//!
//! ## 这个 crate 做了什么
//!
//! Rust 程序在**主机上运行**，用类型安全的 API 描述固件，输出 8051 汇编源码，
//! 然后复用现有工具链汇编、链接、打包、并在模拟器里验证：
//!
//! ```text
//!   main.rs  ──►  mcs51-codegen 生成汇编  ──►  mcs51-asm  ──►  mcs51-ld  ──►  .ihx
//!   (rustc 编译)      (类型安全 API)         (汇编器)       (链接器)      (烧录)
//! ```
//!
//! 好处是真实的：`rustc` 做类型检查、IDE 有补全和跳转、`cargo test` 能测、
//! 端口和引脚是编译期常量而不是字符串。
//!
//! 代价也要说清楚：**跑在芯片上的仍然是 8051 机器码，不是 Rust**；
//! 目标机的控制流（跳转）需要显式表达，因为 Rust 的 `if`/`while` 是在**主机上**
//! 求值的，不会变成目标机的条件跳转。
//!
//! ## 模块状态
//!
//! 板级配置（[`Board`]）已经落地，因为无论后面走哪条路它都要用。
//! 指令发射器（`Task`）与 [`App`] 待定，取决于下面这个选择：
//!
//! | 方案 | 做法 | 代价 | 跑在芯片上的 |
//! |---|---|---|---|
//! | A | Rust eDSL 生成器 | 天 | 8051 机器码 |
//! | B | Rust 子集编译器（不碰 LLVM） | 周 | **真正的 Rust 语义** |
//! | C | 给 LLVM 写 MCS-51 后端 | 月～年，且要 fork LLVM | 真正的 Rust |
//!
//! 详见仓库 `docs/rust-on-8051.md`。

#![forbid(unsafe_code)]
#![warn(missing_docs)]

mod app;
mod emit;
pub mod hal;

/// 芯片标准库：SFR 与位的定义（Keil `reg52.h` / STC 头文件的 Rust 版）。
///
/// eDSL 里的引脚常量（`P35` 等）和这里描述的是同一批硬件，
/// 需要查某个寄存器的地址或说明时直接用它：
///
/// ```
/// use mcs51_codegen::chips;
/// assert_eq!(chips::reg52::TMOD.addr, 0x89);
/// assert_eq!(chips::stc89c5xrc::P4.addr, 0xE8);
/// ```
pub use mcs51_chips as chips;

pub use app::{App, Firmware};
pub use emit::{Bit, Label, Reg, Routine, Task};
pub use hal::{Console, Gpio, Ticker, Timer, TimerId, Uart};

// 硬件定义（`P35`、`TR0`、`TMOD`…）不在这个 crate 里，来自 `mcs51_chips`。
// 见 `prelude` 的文档说明。

/// 常用导入 —— **只有 eDSL，没有硬件定义**。
///
/// 硬件（`P35`、`TR0`、`TMOD`、`ISP_DATA`…）来自芯片标准库，单独导入：
///
/// ```
/// use mcs51_chips::stc89c5xrc::*;   // 这颗芯片上有哪些寄存器、每个位叫什么
/// use mcs51_codegen::prelude::*;        // App / Task / Bit / Reg / Label
/// ```
///
/// **为什么这里不 re-export 硬件**：那会和 `mcs51_chips::*` 撞名
/// （`TR0`、`TF0`、`EA`… 两边都有），两个 glob 一起用就变成歧义。
/// 分层清楚一点：芯片库管"有哪些寄存器"，eDSL 管"怎么生成代码"。
pub mod prelude {
    pub use crate::{
        App, Bit, Board, Console, Firmware, Gpio, Label, Reg, Routine, Task, Ticker, Timer,
        TimerId, Uart,
    };
}

/// 板级参数。无论走 eDSL 还是自研编译器，这些都要传给后端。
///
/// eDSL 不认识任何具体芯片 —— 那是 `mcs51-chips` 的事。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Board {
    /// 工程名，用作产物文件名。
    pub name: String,
    /// 器件型号，只用于文档和尺寸报告。
    pub device: String,
    /// 外部晶振频率，单位 Hz。8051 的定时器都从这里推。
    pub crystal_hz: u32,
    /// 串口波特率。只用于文档和模拟器的串口时序，固件本身不依赖它。
    pub uart_baud: u32,
}

impl Default for Board {
    fn default() -> Self {
        Self {
            name: "firmware".into(),
            device: "STC89C52RC".into(),
            // 11.0592 MHz 是 8051 的标准晶振：它让 9600 波特率精确整除。
            // 12 MHz 下 9600 只能到 10416（+8.5%）。
            crystal_hz: 11_059_200,
            uart_baud: 9600,
        }
    }
}

impl Board {
    /// 工程名。
    pub fn named(mut self, name: impl Into<String>) -> Self {
        self.name = name.into();
        self
    }

    /// 器件型号（一般直接写 `chips::stc89c5xrc::CHIP.name`，别手抄）。
    pub fn device(mut self, name: impl Into<String>) -> Self {
        self.device = name.into();
        self
    }

    /// 晶振频率。
    pub fn crystal(mut self, hz: u32) -> Self {
        self.crystal_hz = hz;
        self
    }

    /// 串口波特率。
    pub fn baud(mut self, baud: u32) -> Self {
        self.uart_baud = baud;
        self
    }

    /// 一个机器周期有多少个时钟（经典 8051 是 12）。
    pub const fn clocks_per_cycle(&self) -> u32 {
        12
    }

    /// 每秒多少个机器周期。
    pub fn machine_cycles_per_second(&self) -> u32 {
        self.crystal_hz / self.clocks_per_cycle()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_board_is_a_standard_8051() {
        let board = Board::default();
        assert_eq!(board.crystal_hz, 11_059_200);
        assert_eq!(board.machine_cycles_per_second(), 921_600);
        assert_eq!(board.clocks_per_cycle(), 12);
    }

    #[test]
    fn board_builders_override_the_defaults() {
        let board = Board::default()
            .named("demo")
            .device("STC89C52RC")
            .crystal(12_000_000)
            .baud(19_200);
        assert_eq!(board.name, "demo");
        assert_eq!(board.device, "STC89C52RC");
        assert_eq!(board.crystal_hz, 12_000_000);
        assert_eq!(board.uart_baud, 19_200);
        assert_eq!(board.machine_cycles_per_second(), 1_000_000);
    }
}
