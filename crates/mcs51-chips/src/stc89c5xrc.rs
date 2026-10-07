//! STC89C5xRC 系列 —— STC 官方头文件 `STC89C5xRC.H` 的 Rust 版。
//!
//! 这一族（STC89C51RC / 52RC / 53RC / 54RD+ / 58RD+ / 510RD+ / 512RD+，
//! 以及对应的 LE 低压版和 STC90C5x）内核是 8052，寄存器完全兼容
//! [`crate::reg52`]，STC 在上面加了 P4 口、ISP/IAP 和看门狗。
//!
//! 所以这里**只定义新增的部分**，`base` 指向 [`crate::reg52::CHIP`]：
//! 查 `TMOD` 会沿链找到 reg52，查 `ISP_DATA` 才落在这里。

use crate::{Bit, Chip, Sfr};

pub use crate::reg52::*;

// 本模块自己也叫 CHIP/SFR/BITS，显式定义会**遮蔽**上面的 glob 重导出，
// 所以 `use stc89c5xrc::*` 拿到的既是全部基础寄存器，又是 STC 的合并视图。
use crate::reg52;

// ---------------------------------------------------------------------------
// STC 增加的字节寄存器
// ---------------------------------------------------------------------------

macro_rules! sfrs {
    ($($name:ident = $addr:literal, $doc:literal;)*) => {
        $(
            #[doc = $doc]
            pub const $name: Sfr = Sfr { name: stringify!($name), addr: $addr, doc: $doc };
        )*

        /// STC 新增的全部字节寄存器。
        pub static SFR: &[Sfr] = &[$($name),*];
    };
}

sfrs! {
    AUXR = 0x8E, "辅助寄存器（STC 扩展；标准 8051 里这个地址是保留的）";
    AUXR1 = 0xA2, "辅助寄存器 1（STC 扩展）";
    SADDR = 0xA9, "从机地址寄存器（80C51Fx/Rx 扩展）";
    IPH = 0xB7, "中断优先级寄存器（高）";
    SADEN = 0xB9, "从机地址掩码寄存器（80C51Fx/Rx 扩展）";
    XICON = 0xC0, "外部中断控制（部分型号）";
    WDT_CONTR = 0xE1, "看门狗控制寄存器";
    ISP_DATA = 0xE2, "ISP/IAP 数据寄存器";
    ISP_ADDRH = 0xE3, "ISP/IAP 地址高字节";
    ISP_ADDRL = 0xE4, "ISP/IAP 地址低字节";
    ISP_CMD = 0xE5, "ISP/IAP 命令寄存器";
    ISP_TRIG = 0xE6, "ISP/IAP 命令触发寄存器";
    ISP_CONTR = 0xE7, "ISP/IAP 控制寄存器";
    P4 = 0xE8, "P4 口特殊寄存器寻址位";
}

// ---------------------------------------------------------------------------
// STC 增加的位
// ---------------------------------------------------------------------------

macro_rules! bits {
    ($($name:ident = $sfr:ident ^ $index:literal, $doc:literal;)*) => {
        $(
            #[doc = $doc]
            pub const $name: Bit = Bit::of(stringify!($name), $sfr, $index, $doc);
        )*

        /// STC 新增的全部位。
        pub static BITS: &[Bit] = &[$($name),*];
    };
}

bits! {
    P40 = P4 ^ 0, "P4.0";
    P41 = P4 ^ 1, "P4.1";
    P42 = P4 ^ 2, "P4.2";
    P43 = P4 ^ 3, "P4.3";
    P44 = P4 ^ 4, "P4.4";
    // 注意：这个脚和 ALE 复用。要用它做 IO，得在 ISP 下载时勾选
    // "ALE 脚用作 P4.5 口"，否则下载器会把它当 ALE。
    P45 = P4 ^ 5, "P4.5（与 ALE 复用；ISP 下载需勾选\"ALE 脚用作 P4.5 口\"）";
    P46 = P4 ^ 6, "P4.6";
    // P4^7 在 STC89C5xRC 上不存在（P4 只有 7 个可用脚）。
}

/// STC89C52RC 的芯片描述。
///
/// 寄存器表对整个 STC89C5xRC 族都成立，差别只在 Flash 大小，
/// 所以这里以最常见的 52RC 为代表。
pub static CHIP: Chip = Chip {
    name: "STC89C52RC",
    vendor: "STC",
    description: "STC89C5xRC 系列（8052 内核 + P4 口 + ISP/IAP + 看门狗）",
    flash_bytes: 8 * 1024,
    idata_bytes: 256,
    xram_bytes: 512,
    base: Some(&reg52::CHIP),
    sfr: SFR,
    bits: BITS,
};

/// 这一族里所有型号的 Flash 大小。
///
/// 寄存器完全一样，只有 Flash 和 RAM 不同 —— 换型号时只需要改这里。
pub static FAMILY: &[(&str, usize, usize)] = &[
    ("STC89C51RC", 4 * 1024, 512),
    ("STC89C52RC", 8 * 1024, 512),
    ("STC89C53RC", 13 * 1024, 512),
    ("STC89C54RD+", 16 * 1024, 1280),
    ("STC89C58RD+", 32 * 1024, 1280),
    ("STC89C510RD+", 42 * 1024, 1280),
    ("STC89C512RD+", 62 * 1024, 1280),
    ("STC89LE52RC", 8 * 1024, 512),
    ("STC90C52RC", 8 * 1024, 512),
    ("STC90C58RD+", 32 * 1024, 1280),
];

/// 按型号名查这一族的 Flash 大小（字节）。
pub fn flash_bytes(model: &str) -> Option<usize> {
    let wanted = model.trim().to_ascii_uppercase();
    FAMILY
        .iter()
        .find(|(name, _, _)| *name == wanted)
        .map(|(_, flash, _)| *flash)
}
