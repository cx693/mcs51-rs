//! eDSL 的硬件定义必须来自芯片标准库，不能是另一份抄本。
//!
//! 曾经 eDSL 里有一套自己的 `Bit` 类型和引脚常量，芯片库是另一套 ——
//! 两份定义迟早会漂移。现在 eDSL 的 `Bit` **就是** `mcs51_chips::Bit`，
//! 引脚是它上面的一层命名别名。

use mcs51_chips::stc89c5xrc::*;
use mcs51_codegen::chips::{reg52, stc89c5xrc};
use mcs51_codegen::prelude::*;

/// 类型相同才能编译通过 —— 这条断言本身就是测试。
fn takes_a_chip_bit(_: mcs51_chips::Bit) {}

#[test]
fn the_esdl_bit_is_the_chip_library_bit() {
    let pin: mcs51_codegen::Bit = P35;
    takes_a_chip_bit(pin);
    assert_eq!(pin.addr, reg52::P35.addr);
    assert_eq!(pin.doc, reg52::P35.doc, "说明也要跟着芯片库走");
}

#[test]
fn every_pin_alias_points_at_the_chip_library_definition() {
    // (eDSL 名字, eDSL 常量, 芯片库常量)
    let pins: [(u8, Bit, Bit); 32] = [
        (0x80, P00, reg52::P00),
        (0x81, P01, reg52::P01),
        (0x82, P02, reg52::P02),
        (0x83, P03, reg52::P03),
        (0x84, P04, reg52::P04),
        (0x85, P05, reg52::P05),
        (0x86, P06, reg52::P06),
        (0x87, P07, reg52::P07),
        (0x90, P10, reg52::P10),
        (0x91, P11, reg52::P11),
        (0x92, P12, reg52::P12),
        (0x93, P13, reg52::P13),
        (0x94, P14, reg52::P14),
        (0x95, P15, reg52::P15),
        (0x96, P16, reg52::P16),
        (0x97, P17, reg52::P17),
        (0xA0, P20, reg52::P20),
        (0xA1, P21, reg52::P21),
        (0xA2, P22, reg52::P22),
        (0xA3, P23, reg52::P23),
        (0xA4, P24, reg52::P24),
        (0xA5, P25, reg52::P25),
        (0xA6, P26, reg52::P26),
        (0xA7, P27, reg52::P27),
        (0xB0, P30, reg52::P30),
        (0xB1, P31, reg52::P31),
        (0xB2, P32, reg52::P32),
        (0xB3, P33, reg52::P33),
        (0xB4, P34, reg52::P34),
        (0xB5, P35, reg52::P35),
        (0xB6, P36, reg52::P36),
        (0xB7, P37, reg52::P37),
    ];
    for (expected, ours, theirs) in pins {
        assert_eq!(ours.addr, expected, "{} 的地址不对", ours.name);
        assert_eq!(
            ours.addr, theirs.addr,
            "{} 和芯片库的 {} 对不上",
            ours.name, theirs.name
        );
        assert_eq!(ours.doc, theirs.doc, "{} 的说明没跟芯片库同步", ours.name);
        // 名字必须和芯片库一致、且非空 —— 拼错引脚名在 eDSL 里是编译错误，
        // 但"名字对了、地址指向别处"编译期看不出来，所以这里逐个核对。
        assert!(!ours.name.is_empty(), "引脚必须有名字");
        assert!(
            ours.name.starts_with('P'),
            "引脚名应该是 P 开头，实际 {}",
            ours.name
        );
    }
}

#[test]
fn the_sfr_bits_come_straight_from_the_chip_library() {
    // 这些是直接 re-export 的，连名字都一样。
    assert_eq!(TR0.addr, reg52::TR0.addr);
    assert_eq!(TF0.addr, reg52::TF0.addr);
    assert_eq!(EA.addr, reg52::EA.addr);
    assert_eq!(CY.addr, reg52::CY.addr);
    assert_eq!(RI.addr, reg52::RI.addr);
    assert_eq!(TR0.name, "TR0");
}

#[test]
fn an_application_can_reach_the_whole_chip_library() {
    // 应用里想用 STC 扩展或查别的芯片，直接走 mcs51_codegen::chips。
    assert_eq!(stc89c5xrc::P4.addr, 0xE8);
    assert_eq!(stc89c5xrc::ISP_DATA.addr, 0xE2);
    let chip = mcs51_codegen::chips::find("STC89C52RC").unwrap();
    assert_eq!(chip.flash_bytes, 8192);
    assert!(chip.bit_named("P35").is_some());
}

/// 应用变量**绝不能**落在 `0x80` 以上。
///
/// 8051 的直接寻址空间里 `0x80..0xFF` 全是 SFR，不是 RAM：`0xD0` 是 PSW，
/// `0xD1..0xD3` 未实现（读回不确定、写入无效）。把变量放进去，
/// `MOV direct` 打到的是 PSW 和一堆不存在的寄存器。
///
/// 这个 bug 真的发生过：内核版呼吸灯的 4 个字节落在 `0xD0..0xD3`，
/// **模拟器里一切正常**（它把 SFR 空洞当普通内存），到真机上 P3.7 完全不动。
#[test]
fn application_variables_stay_in_directly_addressable_ram() {
    // 8051 的直接寻址空间里 `0x80..0xFF` 全是 SFR，不是 RAM：`0xD0` 是 PSW，
    // `0xD1..0xD3` 未实现（读回不确定、写入无效）。把变量放进去，
    // `MOV direct` 打到的其实是 PSW 和一堆不存在的寄存器 —— 模拟器可能碰巧
    // 跑得通，真机上必然废。应用区从 0x20 起，到栈底 0x30 为止。
    let mut app = App::new(Board::default());
    let base = app.statics(4);
    assert_eq!(base, 0x20, "应用变量区从 0x20 开始");
    assert!(base < 0x80, "应用变量不能落在 SFR 区：{base:#04X}");
}

#[test]
fn more_static_ram_than_the_window_is_a_build_error() {
    // 0x20..0x30 正好 16 个字节，用满应该能构建。
    let mut app = App::new(Board::default());
    let _ = app.statics(16);
    app.main(|t| {
        t.forever(|t| {
            t.nop();
        });
    });
    assert!(app.finish().is_ok(), "用满 16 字节应该合法");

    // 多要一个字节就必须报错，而且要说清楚哪些区间是被占的。
    let mut app = App::new(Board::default());
    let _ = app.statics(17);
    app.main(|t| {
        t.forever(|t| {
            t.nop();
        });
    });
    let error = app.finish().unwrap_err();
    assert!(error.contains("0x30"), "要说明栈的位置：{error}");
    assert!(error.contains("SFR"), "要说明 0x80 以上是 SFR：{error}");
    assert!(error.contains("XDATA"), "要给出出路：{error}");
}

#[test]
fn the_generated_code_never_direct_addresses_an_sfr_as_a_variable() {
    // 端到端：真的生成一份固件，检查里面没有把 0x80..0xFF 当变量操作的指令。
    //
    // 这不是万能的（立即数 `#0xB5` 不会匹配，因为模式要求前面是操作码），
    // 但足以抓住"变量被放到 SFR 区"这一类问题。
    let mut app = App::new(Board::default());
    let state = app.statics(4);
    app.main(move |t| {
        t.mov_direct_imm(state, 0);
        t.forever(|t| {
            t.inc_direct(state);
            t.mov_a_direct(state);
            t.mov_direct_a(state + 1);
        });
    });
    let firmware = app.finish().expect("应该能生成");

    // 变量必须落在 0x20..0x2F。
    for line in firmware.assembly.lines() {
        let line = line.trim();
        for mnemonic in [
            "MOV", "INC", "DEC", "CJNE", "DJNZ", "ADD", "SUBB", "ANL", "ORL", "XRL", "PUSH", "POP",
        ] {
            let Some(rest) = line.strip_prefix(mnemonic) else {
                continue;
            };
            for operand in rest.split(',') {
                let operand = operand.trim();
                let Some(hex) = operand.strip_prefix("0x") else {
                    continue;
                };
                let Ok(address) = u8::from_str_radix(hex, 16) else {
                    continue;
                };
                // 0x80..0xFF 是 SFR 区；寄存器组 PUSH 0x00..0x07 是合法的。
                assert!(
                    !(0x80..=0xFF).contains(&address),
                    "指令 `{line}` 在直接寻址 SFR 区（{address:#04X}）—— \
                     变量必须放在 0x80 以下"
                );
            }
        }
    }
}
