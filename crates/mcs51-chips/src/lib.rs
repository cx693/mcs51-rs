//! # mcs51-chips —— 8051 芯片标准库
//!
//! Keil 的 `reg52.h` / `STC89C5xRC.h` 是 8051 世界事实上的"芯片标准库"：
//! 把每个特殊功能寄存器（SFR）和每个可位寻址的位，变成一个能写进代码的名字。
//! 这个 crate 是它们的 Rust 版，并且**把注释也一起搬过来了** ——
//! 头文件里那些「TF0 是定时器 0 溢出标志」才是真正有价值的部分。
//!
//! ## 和 `mcs51-isa` 的分工
//!
//! | crate | 管什么 |
//! |---|---|
//! | `mcs51-isa` | **指令集**：操作码、编码、以及一套通用的 SFR 名字表（给反汇编用） |
//! | `mcs51-chips` | **具体芯片**：这颗片子上有哪些寄存器、每个位叫什么、注释是什么 |
//!
//! ## 怎么用
//!
//! ```
//! use mcs51_chips::{stc89c5xrc, reg52};
//!
//! // 直接引用具体寄存器
//! assert_eq!(reg52::TMOD.addr, 0x89);
//! assert_eq!(reg52::TR0.addr, 0x8C);       // TCON(0x88) + 4
//! assert_eq!(stc89c5xrc::P4.addr, 0xE8);   // STC 才有的 P4 口
//!
//! // 或者按芯片名查
//! let chip = mcs51_chips::find("STC89C52RC").unwrap();
//! assert_eq!(chip.flash_bytes, 8192);
//! assert!(chip.sfr_named("ISP_DATA").is_some());
//! assert_eq!(chip.bit_named("TR0").unwrap().addr, 0x8C);
//! ```
//!
//! ## 位地址是怎么来的
//!
//! 8051 里可位寻址的 SFR 地址都是 8 的倍数且 ≥ 0x80，位地址就是
//! `SFR 地址 + 位序号`。这个 crate **不手写位地址**，而是用
//! [`Bit::of`] 从 SFR 推出来 —— 写错序号是不可能的，8 的倍数这条规则
//! 也有测试盯着。
//!
//! ## 加一颗新芯片
//!
//! 1. 照着 `reg52.rs` 新建 `src/你的芯片.rs`，定义 `pub static SFR: &[Sfr]`
//!    和 `pub static BITS: &[Bit]`，以及一个 `pub static CHIP: Chip`；
//! 2. 如果它是 8051/8052 的扩展（绝大多数都是），把 `base` 指向 `reg52::CHIP`，
//!    这样基础寄存器不用重复定义；
//! 3. 在 [`CHIPS`] 里注册；
//! 4. `cargo test -p mcs51-chips` —— 地址冲突、位地址推导、名字合法性都会被检查。

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod reg52;
pub mod stc89c5xrc;

/// 一个 8 位特殊功能寄存器。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Sfr {
    /// 汇编里用的名字，例如 `TMOD`。
    pub name: &'static str,
    /// 直接地址，例如 `0x89`。
    pub addr: u8,
    /// 中文说明（一般直接来自厂商头文件）。
    pub doc: &'static str,
}

/// 一个可位寻址的位。
///
/// 位地址空间是统一的：`0x00..0x80` 落在内部 RAM 的 `0x20..0x2F`，
/// `0x80..0xFF` 落在 SFR 区。`SETB`/`CLR`/`JB`/`MOV C,bit` 对两者一视同仁。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Bit {
    /// 汇编里用的名字，例如 `TR0` 或 `P35`。
    pub name: &'static str,
    /// 位地址，例如 `0x8C`。
    pub addr: u8,
    /// 中文说明。
    pub doc: &'static str,
}

impl Bit {
    /// 从一个 SFR 和位序号推出位地址。
    ///
    /// 这是唯一的构造方式 —— 位地址不手写，所以不可能和 SFR 对不上。
    pub const fn of(name: &'static str, sfr: Sfr, index: u8, doc: &'static str) -> Bit {
        Bit {
            name,
            addr: sfr.addr + index,
            doc,
        }
    }
}

impl Bit {
    /// 给同一个物理位起个别名。
    ///
    /// 位地址**跟着原定义走**，所以别名不可能和真身漂移 ——
    /// 例如 eDSL 习惯把 Keil 的 `P35` 写成 `P3_5`，地址仍来自这里。
    pub const fn alias(name: &'static str, other: Bit) -> Bit {
        Bit {
            name,
            addr: other.addr,
            doc: other.doc,
        }
    }
}

impl Sfr {
    /// 这个 SFR 能不能按位寻址。
    ///
    /// 规则：地址 ≥ 0x80 且是 8 的倍数（0x80、0x88、0x90 …）。
    pub const fn is_bit_addressable(self) -> bool {
        self.addr >= 0x80 && self.addr % 8 == 0
    }

    /// 这个 SFR 的第 `index` 位。
    pub const fn bit(self, name: &'static str, index: u8, doc: &'static str) -> Bit {
        Bit::of(name, self, index, doc)
    }
}

impl From<Sfr> for u8 {
    /// 把寄存器直接当地址用。
    ///
    /// 这样 eDSL 的 `mov_direct_imm(TMOD, 1)` 可以直接吃 `reg52::TMOD`，
    /// 不用在应用里写一层 `pub const TMOD: u8 = ...addr` 的适配常量。
    fn from(sfr: Sfr) -> Self {
        sfr.addr
    }
}

impl From<Bit> for u8 {
    /// 把位直接当位地址用。
    fn from(bit: Bit) -> Self {
        bit.addr
    }
}

/// 一颗芯片的完整描述。
#[derive(Debug, Clone, Copy)]
pub struct Chip {
    /// 型号名，和 `mcs51.toml` 里的 `device` 一致。
    pub name: &'static str,
    /// 厂商。
    pub vendor: &'static str,
    /// 一句话说明。
    pub description: &'static str,
    /// Flash 字节数。
    pub flash_bytes: usize,
    /// 内部 RAM 字节数（含只能用间接寻址的高 128 字节）。
    pub idata_bytes: usize,
    /// 片内 XRAM 字节数（`MOVX` 访问）。
    pub xram_bytes: usize,
    /// 被扩展的基类芯片；查不到的名字会沿这条链继续找。
    pub base: Option<&'static Chip>,
    /// 这颗芯片**新增**的 SFR（不含继承来的）。
    pub sfr: &'static [Sfr],
    /// 这颗芯片**新增**的位。
    pub bits: &'static [Bit],
}

impl Chip {
    /// 按名字查一个 SFR，包括从基类继承来的。
    pub fn sfr_named(&self, name: &str) -> Option<Sfr> {
        if let Some(found) = self.sfr.iter().find(|s| s.name == name) {
            return Some(*found);
        }
        self.base.and_then(|base| base.sfr_named(name))
    }

    /// 按名字查一个位，包括从基类继承来的。
    pub fn bit_named(&self, name: &str) -> Option<Bit> {
        if let Some(found) = self.bits.iter().find(|b| b.name == name) {
            return Some(*found);
        }
        self.base.and_then(|base| base.bit_named(name))
    }

    /// 这颗芯片（含继承）的全部 SFR，基类在前。
    pub fn all_sfr(&self) -> Vec<Sfr> {
        let mut out = self.base.map(|b| b.all_sfr()).unwrap_or_default();
        out.extend_from_slice(self.sfr);
        out
    }

    /// 这颗芯片（含继承）的全部位。
    pub fn all_bits(&self) -> Vec<Bit> {
        let mut out = self.base.map(|b| b.all_bits()).unwrap_or_default();
        out.extend_from_slice(self.bits);
        out
    }

    /// 按名字找这颗芯片上的一个 SFR 或位（用于 `SETB`/`MOV` 之类的符号解析）。
    pub fn symbol_named(&self, name: &str) -> Option<(u8, bool)> {
        if let Some(sfr) = self.sfr_named(name) {
            return Some((sfr.addr, false));
        }
        self.bit_named(name).map(|bit| (bit.addr, true))
    }
}

/// 所有已知芯片。查名字大小写不敏感。
pub static CHIPS: &[Chip] = &[stc89c5xrc::CHIP];

/// 按型号名找芯片（大小写不敏感，也接受若干别名）。
pub fn find(name: &str) -> Option<&'static Chip> {
    let wanted = name.trim().to_ascii_uppercase();
    CHIPS
        .iter()
        .find(|chip| chip.name.to_ascii_uppercase() == wanted)
        .or_else(|| {
            // STC89C5xRC 是一整族：51/52/53/54/58/510/512 都是同一套寄存器。
            wanted.starts_with("STC89C5").then_some(&stc89c5xrc::CHIP)
        })
        .or_else(|| wanted.starts_with("STC90C5").then_some(&stc89c5xrc::CHIP))
        .or_else(|| wanted.starts_with("8052").then_some(&reg52::CHIP))
}

/// 所有芯片的型号名，给 `mcs51 --list-chips` 之类的命令用。
pub fn names() -> Vec<&'static str> {
    CHIPS.iter().map(|chip| chip.name).collect()
}
