//! 芯片定义的正确性约束。
//!
//! 芯片标准库最怕的是**悄悄写错**：地址抄错一位、位序号偏一格、两个名字撞在
//! 同一个地址上。这些都不会有编译错误，只会让固件在真机上莫名其妙地不工作。
//! 所以这里把能机械检查的都检查掉。

use std::collections::HashMap;

use mcs51_chips::{CHIPS, Chip, find, names, reg52, stc89c5xrc};

/// 汇编标识符：字母/下划线开头，后面是字母数字下划线。
/// 芯片库里的名字必须能直接写进汇编，否则 `SETB TR0` 之类会解析不了。
fn is_assembler_identifier(name: &str) -> bool {
    let mut chars = name.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

#[test]
fn every_register_and_bit_has_a_usable_name() {
    for chip in CHIPS {
        for sfr in chip.all_sfr() {
            assert!(
                is_assembler_identifier(sfr.name),
                "{} 的 SFR 名字 `{}` 不能直接写进汇编",
                chip.name,
                sfr.name
            );
            assert!(
                !sfr.doc.is_empty(),
                "{} 的 {} 没有说明",
                chip.name,
                sfr.name
            );
        }
        for bit in chip.all_bits() {
            assert!(
                is_assembler_identifier(bit.name),
                "{} 的位名字 `{}` 不能直接写进汇编",
                chip.name,
                bit.name
            );
            assert!(
                !bit.doc.is_empty(),
                "{} 的 {} 没有说明",
                chip.name,
                bit.name
            );
        }
    }
}

#[test]
fn no_two_registers_share_an_address() {
    for chip in CHIPS {
        let mut seen: HashMap<u8, &str> = HashMap::new();
        for sfr in chip.all_sfr() {
            // 第二功能名指向同一个物理位是合法的，SFR 之间不应该。
            if let Some(previous) = seen.insert(sfr.addr, sfr.name)
                && previous != sfr.name
            {
                panic!(
                    "{}：{} 和 {} 都定义在地址 {:#04X}",
                    chip.name, previous, sfr.name, sfr.addr
                );
            }
        }
    }
}

#[test]
fn bit_addresses_are_derived_not_hand_written() {
    // 规则：可位寻址的 SFR 地址是 8 的倍数且 ≥ 0x80，位地址 = SFR 地址 + 位序号。
    // 因为位是用 `Bit::of` 推出来的，这条应该是恒等式；测试的作用是防止
    // 以后有人为了"方便"改成手写地址。
    for chip in CHIPS {
        for bit in chip.all_bits() {
            let base = bit.addr & 0xF8;
            let index = bit.addr & 0x07;
            assert!(
                base >= 0x80,
                "{} 的位地址 {:#04X} 落在 SFR 区之外",
                chip.name,
                bit.addr
            );
            let sfr = chip
                .all_sfr()
                .into_iter()
                .find(|s| s.addr == base)
                .unwrap_or_else(|| {
                    panic!(
                        "{} 的位 {}（{:#04X}）指向的 SFR {:#04X} 不存在",
                        chip.name, bit.name, bit.addr, base
                    )
                });
            assert!(
                sfr.is_bit_addressable(),
                "{}：{} 不是可位寻址的 SFR，但它有位定义",
                chip.name,
                sfr.name
            );
            assert_eq!(
                bit.addr,
                sfr.addr + index,
                "{} 的 {} 位地址和 {}.{} 对不上",
                chip.name,
                bit.name,
                sfr.name,
                index
            );
        }
    }
}

#[test]
fn the_reg52_table_matches_the_keil_header() {
    // 逐个核对 Keil reg52.h 里的地址。抄错一位这里就会红。
    let expected: &[(&str, u8)] = &[
        ("P0", 0x80),
        ("SP", 0x81),
        ("DPL", 0x82),
        ("DPH", 0x83),
        ("PCON", 0x87),
        ("TCON", 0x88),
        ("TMOD", 0x89),
        ("TL0", 0x8A),
        ("TL1", 0x8B),
        ("TH0", 0x8C),
        ("TH1", 0x8D),
        ("P1", 0x90),
        ("SCON", 0x98),
        ("SBUF", 0x99),
        ("P2", 0xA0),
        ("IE", 0xA8),
        ("P3", 0xB0),
        ("IP", 0xB8),
        ("T2CON", 0xC8),
        ("RCAP2L", 0xCA),
        ("RCAP2H", 0xCB),
        ("TL2", 0xCC),
        ("TH2", 0xCD),
        ("PSW", 0xD0),
        ("ACC", 0xE0),
        ("B", 0xF0),
    ];
    for (name, addr) in expected {
        let found = reg52::CHIP
            .sfr_named(name)
            .unwrap_or_else(|| panic!("reg52 里缺 {name}"));
        assert_eq!(found.addr, *addr, "{name} 的地址不对");
    }
}

#[test]
fn the_bit_table_matches_the_keil_header() {
    // 核对 reg52.h 里所有 sbit 的位地址。
    let expected: &[(&str, u8)] = &[
        // PSW
        ("CY", 0xD7),
        ("AC", 0xD6),
        ("F0", 0xD5),
        ("RS1", 0xD4),
        ("RS0", 0xD3),
        ("OV", 0xD2),
        ("P", 0xD0),
        // TCON
        ("TF1", 0x8F),
        ("TR1", 0x8E),
        ("TF0", 0x8D),
        ("TR0", 0x8C),
        ("IE1", 0x8B),
        ("IT1", 0x8A),
        ("IE0", 0x89),
        ("IT0", 0x88),
        // IE
        ("EA", 0xAF),
        ("ET2", 0xAD),
        ("ES", 0xAC),
        ("ET1", 0xAB),
        ("EX1", 0xAA),
        ("ET0", 0xA9),
        ("EX0", 0xA8),
        // IP
        ("PT2", 0xBD),
        ("PS", 0xBC),
        ("PT1", 0xBB),
        ("PX1", 0xBA),
        ("PT0", 0xB9),
        ("PX0", 0xB8),
        // SCON
        ("SM0", 0x9F),
        ("SM1", 0x9E),
        ("SM2", 0x9D),
        ("REN", 0x9C),
        ("TB8", 0x9B),
        ("RB8", 0x9A),
        ("TI", 0x99),
        ("RI", 0x98),
        // T2CON
        ("TF2", 0xCF),
        ("EXF2", 0xCE),
        ("RCLK", 0xCD),
        ("TCLK", 0xCC),
        ("EXEN2", 0xCB),
        ("TR2", 0xCA),
        ("C_T2", 0xC9),
        ("CP_RL2", 0xC8),
        // P3 的第二功能
        ("RXD", 0xB0),
        ("TXD", 0xB1),
        ("INT0", 0xB2),
        ("INT1", 0xB3),
        ("T0", 0xB4),
        ("T1", 0xB5),
        ("WR", 0xB6),
        ("RD", 0xB7),
        // P1 的第二功能
        ("T2", 0x90),
        ("T2EX", 0x91),
    ];
    for (name, addr) in expected {
        let found = reg52::CHIP
            .bit_named(name)
            .unwrap_or_else(|| panic!("reg52 里缺 {name}"));
        assert_eq!(found.addr, *addr, "{name} 的位地址不对");
    }
}

#[test]
fn the_port_pins_cover_all_four_ports() {
    for port in 0..4u8 {
        for bit in 0..8u8 {
            let name = format!("P{port}{bit}");
            let found = reg52::CHIP
                .bit_named(&name)
                .unwrap_or_else(|| panic!("缺引脚 {name}"));
            assert_eq!(found.addr, 0x80 + port * 0x10 + bit, "{name} 的位地址不对");
        }
    }
}

#[test]
fn the_stc_chip_inherits_reg52_and_adds_its_own() {
    let stc = &stc89c5xrc::CHIP;

    // 继承来的：沿 base 链能找到。
    assert_eq!(stc.sfr_named("TMOD").unwrap().addr, 0x89);
    assert_eq!(stc.bit_named("TR0").unwrap().addr, 0x8C);
    assert_eq!(stc.bit_named("P35").unwrap().addr, 0xB5);

    // STC 自己的。
    assert_eq!(stc.sfr_named("P4").unwrap().addr, 0xE8);
    assert_eq!(stc.sfr_named("ISP_DATA").unwrap().addr, 0xE2);
    assert_eq!(stc.sfr_named("WDT_CONTR").unwrap().addr, 0xE1);
    assert_eq!(stc.sfr_named("AUXR").unwrap().addr, 0x8E);
    assert_eq!(stc.bit_named("P45").unwrap().addr, 0xED);

    // P45 和 ALE 复用，文档里必须写清楚这一点。
    assert!(stc.bit_named("P45").unwrap().doc.contains("ALE"));

    // 继承来的部分不能在子表里重复定义。
    for sfr in stc89c5xrc::SFR {
        assert!(
            reg52::CHIP.sfr_named(sfr.name).is_none(),
            "{} 在 reg52 里已经有了，不该在 STC 表里重复定义",
            sfr.name
        );
    }
    for bit in stc89c5xrc::BITS {
        assert!(
            reg52::CHIP.bit_named(bit.name).is_none(),
            "{} 在 reg52 里已经有了，不该在 STC 表里重复定义",
            bit.name
        );
    }

    // 合并视图应该比任何一边都大。
    assert!(stc.all_sfr().len() > reg52::CHIP.all_sfr().len());
    assert!(stc.all_bits().len() > reg52::CHIP.all_bits().len());
}

#[test]
fn the_stc_family_lists_the_flash_sizes() {
    assert_eq!(stc89c5xrc::flash_bytes("STC89C52RC"), Some(8192));
    assert_eq!(stc89c5xrc::flash_bytes("stc89c54rd+"), Some(16 * 1024));
    assert_eq!(stc89c5xrc::flash_bytes("STC89C512RD+"), Some(62 * 1024));
    assert_eq!(stc89c5xrc::flash_bytes("不存在"), None);
}

#[test]
fn chips_can_be_found_by_model_name() {
    assert!(names().contains(&"STC89C52RC"));

    let by_exact = find("STC89C52RC").expect("按型号名应该能找到");
    assert_eq!(by_exact.flash_bytes, 8192);
    assert_eq!(by_exact.vendor, "STC");

    // 大小写不敏感。
    assert!(find("stc89c52rc").is_some());

    // 同族型号都映射到同一套定义（寄存器一样，只有 Flash 不同）。
    for model in ["STC89C51RC", "STC89C58RD+", "STC90C52RC"] {
        let chip = find(model).unwrap_or_else(|| panic!("{model} 应该能找到"));
        assert_eq!(chip.name, "STC89C52RC");
    }

    // 通用 8052 也能查。
    assert!(find("8052").is_some());

    assert!(find("PIC16F84").is_none());
}

#[test]
fn symbol_lookup_tells_registers_from_bits() {
    let stc = &stc89c5xrc::CHIP;
    // (地址, 是不是位)
    assert_eq!(stc.symbol_named("TMOD"), Some((0x89, false)));
    assert_eq!(stc.symbol_named("TR0"), Some((0x8C, true)));
    assert_eq!(stc.symbol_named("P35"), Some((0xB5, true)));
    assert_eq!(stc.symbol_named("不存在"), None);
}

#[test]
fn only_八的倍数_and_high_enough_registers_are_bit_addressable() {
    assert!(reg52::P0.is_bit_addressable());
    assert!(reg52::TCON.is_bit_addressable());
    assert!(reg52::PSW.is_bit_addressable());
    assert!(stc89c5xrc::P4.is_bit_addressable());
    // 这些不是 8 的倍数，不能按位寻址。
    assert!(!reg52::TMOD.is_bit_addressable());
    assert!(!reg52::SBUF.is_bit_addressable());
    assert!(!reg52::SP.is_bit_addressable());
}

#[test]
fn the_chip_descriptor_agrees_with_the_family_table() {
    // `CHIP.flash_bytes` 是代表型号的值，必须和 FAMILY 表里的一致，
    // 否则 `mcs51 flash --flash-size` 会拒绝合法的镜像。
    let from_table = stc89c5xrc::flash_bytes(stc89c5xrc::CHIP.name).expect("代表型号要在家族表里");
    assert_eq!(stc89c5xrc::CHIP.flash_bytes, from_table);
}

/// 这个测试不检查对错，只把库的规模打出来，方便一眼看出改动的影响。
#[test]
fn the_library_covers_what_the_headers_do() {
    let mut report = Vec::new();
    for chip in CHIPS {
        report.push(format!(
            "{}: {} 个寄存器，{} 个位",
            chip.name,
            chip.all_sfr().len(),
            chip.all_bits().len()
        ));
    }
    // reg52.h 里有 26 个 sfr、约 100 个 sbit；STC 头文件再加 14 个 sfr、7 个 sbit。
    assert!(
        reg52::CHIP.all_sfr().len() >= 26,
        "reg52 的寄存器数量不对：{}",
        reg52::CHIP.all_sfr().len()
    );
    assert!(
        reg52::CHIP.all_bits().len() >= 100,
        "reg52 的位数量不对：{}",
        reg52::CHIP.all_bits().len()
    );
    assert!(
        stc89c5xrc::SFR.len() >= 14,
        "STC 的寄存器数量不对：{}",
        stc89c5xrc::SFR.len()
    );
    eprintln!("{}", report.join("\n"));
}

// 让 `use` 不报未使用。
#[allow(dead_code)]
fn _unused(_: &Chip) {}
