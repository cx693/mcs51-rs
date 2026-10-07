//! End-to-end assembler tests, including a cross-check of the assembled bytes
//! against the independent `i8051` emulator's disassembler.

use mcs51_asm::{Options, RelocKind, SymbolLoc, assemble};
use mcs51_isa::{Instruction, decode};

fn asm(source: &str) -> mcs51_asm::AssembleOutput {
    let out = assemble(source, &Options::new("test.asm"));
    assert!(
        out.diagnostics.is_empty(),
        "unexpected diagnostics:\n{}",
        out.render_diagnostics()
    );
    out
}

#[test]
fn assembles_a_two_instruction_program() {
    let out = asm("
        .area CODE (ABS)
        .org  0x0100
start:  MOV   A, #0x42
        SJMP  start
    ");
    assert_eq!(
        out.object.section("CODE").unwrap().data,
        vec![0x74, 0x42, 0x80, 0xFC]
    );
    assert_eq!(out.object.section("CODE").unwrap().org, Some(0x0100));
}

#[test]
fn pads_interrupt_vectors_with_gaps() {
    let out = asm("
        .area HOME (ABS)
        .org  0x0000
        LJMP  start
        .org  0x000B
        LJMP  tick
        .area CODE (ABS)
        .org  0x0030
start:  NOP
tick:   RETI
    ");
    let home = out.object.section("HOME").unwrap();
    assert_eq!(&home.data[0..3], &[0x02, 0x00, 0x30]);
    assert_eq!(&home.data[3..11], &[0xFF; 8]);
    assert_eq!(&home.data[11..14], &[0x02, 0x00, 0x31]); // `tick` is one byte after `start`
    assert_eq!(home.data.len(), 14);
}

#[test]
fn emits_relocations_for_relocatable_sections() {
    let out = asm("
        .area CODE
main:   MOV   DPTR, #message
        SJMP  main
message:.db   \"hi\", 0
    ");
    let code = out.object.section("CODE").unwrap();
    // The branch inside the section is resolved; the address of `message` is not.
    assert_eq!(
        code.data,
        vec![0x90, 0x00, 0x00, 0x80, 0xFB, b'h', b'i', 0x00]
    );
    assert_eq!(code.relocs.len(), 1);
    let reloc = &code.relocs[0];
    assert_eq!(reloc.kind, RelocKind::Abs16);
    assert_eq!(reloc.symbol, "message");
    assert_eq!(reloc.offset, 1);
    assert!(out.object.externs.is_empty());
}

#[test]
fn undefined_symbols_become_externs() {
    let out = asm("
        .area CODE
        LCALL helper
        LJMP  helper
    ");
    assert_eq!(out.object.externs, vec!["helper".to_string()]);
    let code = out.object.section("CODE").unwrap();
    assert_eq!(code.relocs.len(), 2);
    assert!(code.relocs.iter().all(|r| r.kind == RelocKind::Abs16));
}

#[test]
fn relative_branches_inside_a_section_are_resolved() {
    let out = asm("
        .area CODE
loop:   NOP
        DJNZ  R0, loop
    ");
    let code = out.object.section("CODE").unwrap();
    // Same section: fully resolved, no relocation.
    assert_eq!(code.data, vec![0x00, 0xD8, 0xFD]);
    assert!(code.relocs.is_empty());
}

#[test]
fn equates_labels_and_local_labels_work() {
    let out = asm("
        COUNT .equ 4
        .area CODE (ABS)
        .org  0x0200
entry:  MOV   R7, #COUNT
.loop:  NOP
        DJNZ  R7, .loop
        SJMP  entry
    ");
    assert_eq!(
        out.object.section("CODE").unwrap().data,
        vec![0x7F, 0x04, 0x00, 0xDF, 0xFD, 0x80, 0xF9]
    );
}

#[test]
fn bit_instructions_and_the_device_header() {
    let out = asm("
        .include \"8051.inc\"
        .area CODE (ABS)
        .org  0x0000
        SETB  TR0
        SETB  EA
        CLR   TF0
        MOV   C, P1_0
        MOV   P1_1, C
    ");
    assert_eq!(
        out.object.section("CODE").unwrap().data,
        vec![0xD2, 0x8C, 0xD2, 0xAF, 0xC2, 0x8D, 0xA2, 0x90, 0x92, 0x91]
    );
}

#[test]
fn expressions_use_low_and_high_and_dollar() {
    let out = asm("
        .area CODE (ABS)
        .org  0x1234
table:  MOV   A, #low(table)
        MOV   A, #high(table)
        MOV   DPTR, #table + 2
        MOV   A, #(table >> 8) & 0xFF
    ");
    let data = &out.object.section("CODE").unwrap().data;
    assert_eq!(
        data,
        &vec![0x74, 0x34, 0x74, 0x12, 0x90, 0x12, 0x36, 0x74, 0x12]
    );
}

#[test]
fn data_directives() {
    let out = asm("
        .area CODE (ABS)
        .org 0x0000
        .db  1, 2, 3
        .db  \"AB\", 0
        .dw  0x1234, 0xABCD
        .ds  3, 0xEE
        .align 4
        .db  0x99
    ");
    let data = &out.object.section("CODE").unwrap().data;
    assert_eq!(
        data,
        &vec![
            1, 2, 3, b'A', b'B', 0, 0x12, 0x34, 0xAB, 0xCD, 0xEE, 0xEE, 0xEE, 0x00, 0x00, 0x00,
            0x99
        ]
    );
}

#[test]
fn reports_out_of_range_branches() {
    let out = assemble(
        "
        .area CODE (ABS)
        .org  0x0000
start:  SJMP  far_away
        .org  0x0200
far_away: NOP
        ",
        &Options::new("test.asm"),
    );
    assert!(!out.is_ok());
    let text = out.render_diagnostics();
    assert!(text.contains("out of range"), "{text}");
}

#[test]
fn reports_unknown_mnemonics_and_directives() {
    let out = assemble(
        "
        FROBNICATE A, B
        .wat 1
        ",
        &Options::new("test.asm"),
    );
    let text = out.render_diagnostics();
    assert!(text.contains("unknown mnemonic"), "{text}");
    assert!(text.contains("unknown directive"), "{text}");
}

#[test]
fn reports_ajmp_page_violations() {
    let out = assemble(
        "
        .area CODE (ABS)
        .org  0x0000
start:  AJMP  other_page
        .org  0x0900
other_page: NOP
        ",
        &Options::new("test.asm"),
    );
    let text = out.render_diagnostics();
    assert!(text.contains("2 KiB page"), "{text}");
}

/// Every instruction the assembler emits must mean the same thing to the
/// independent emulator.
#[test]
fn assembled_bytes_decode_identically_in_the_emulator() {
    let source = "
        .include \"8051.inc\"
        .area CODE (ABS)
        .org  0x0000
        NOP
        ADD   A, #0x10
        ADD   A, 0x30
        ADD   A, @R0
        ADD   A, R1
        MOV   DPTR, #0x1234
        MOVC  A, @A+DPTR
        MOVX  A, @DPTR
        MOVX  @R1, A
        CJNE  R2, #5, next
next:   DJNZ  R3, next
        JB    TR0, next
        ANL   C, /TR1
        ORL   C, TF0
        XCHD  A, @R0
        SWAP  A
        DA    A
        PUSH  0x30
        POP   ACC
        RETI
    ";
    let out = asm(source);
    let data = &out.object.section("CODE").unwrap().data;

    let mut addr = 0usize;
    while addr < data.len() {
        let ours = decode(addr as u16, &data[addr..]).expect("assembled byte must decode");
        let theirs = i8051::Instruction::decode_from_bytes(addr as u32, &data[addr..]);
        assert_eq!(
            ours.mnemonic.as_str(),
            theirs.mnemonic().as_str(),
            "at {addr:#06X}: {ours} vs emulator {}",
            theirs.as_string()
        );
        assert_eq!(ours.len() as usize, theirs.len());
        addr += ours.len() as usize;
    }
    assert_eq!(addr, data.len());
}

#[test]
fn gather_symbols_with_addresses() {
    let out = asm("
        .area CODE (ABS)
        .org  0x0030
main:   NOP
sub:    RET
        .area DATA (ABS)
        .org  0x30
var:    .ds  2
        .area BIT
flag    .bit 0x20
    ");
    let symbol = |name: &str| out.object.symbol(name).expect(name);
    match &symbol("main").loc {
        SymbolLoc::Section { section, offset } => {
            assert_eq!(section, "CODE");
            assert_eq!(*offset, 0);
        }
        other => panic!("unexpected {other:?}"),
    }
    match &symbol("flag").loc {
        SymbolLoc::Absolute(v) => assert_eq!(*v, 0x20),
        other => panic!("unexpected {other:?}"),
    }
}

#[test]
fn listing_records_bytes_per_line() {
    let out = asm("
        .area CODE (ABS)
        .org 0x0000
        NOP
        MOV A, #1
    ");
    let listing: Vec<_> = out.listing.iter().filter(|l| !l.bytes.is_empty()).collect();
    assert_eq!(listing.len(), 2);
    assert_eq!(listing[0].bytes, vec![0x00]);
    assert_eq!(listing[1].bytes, vec![0x74, 0x01]);
    assert_eq!(listing[0].addr, Some(0));
}

/// Every instruction in the ISA table must have a source spelling that the
/// assembler accepts and encodes to the same length.
#[test]
fn every_defined_opcode_can_be_written_in_source() {
    let mut source = String::from(".area CODE (ABS)\n.org 0x0000\n");
    let mut expected = 0usize;
    for def in mcs51_isa::OPCODES {
        for n in 0..def.variant_count() {
            let opcode = def.variant(n);
            let len = mcs51_isa::instruction_length(opcode) as usize;
            let bytes = [opcode, 0x00, 0x00];
            let Ok(ins) = decode(0x0000, &bytes[..len]) else {
                continue;
            };
            source.push_str(&render(ins));
            source.push('\n');
            expected += 1;
        }
    }
    let out = assemble(&source, &Options::new("all.asm"));
    assert!(
        out.is_ok(),
        "every opcode should be expressible in source:\n{}",
        out.render_diagnostics()
    );
    let emitted: usize = out
        .listing
        .iter()
        .filter(|line| !line.bytes.is_empty())
        .count();
    assert_eq!(
        emitted, expected,
        "every instruction must assemble to a line"
    );

    // And the whole stream must decode back to the same instruction sequence.
    let data = &out.object.section("CODE").unwrap().data;
    let mut addr = 0usize;
    let mut decoded = 0usize;
    while addr < data.len() {
        let ins = decode(addr as u16, &data[addr..]).expect("assembled bytes must decode");
        addr += ins.len() as usize;
        decoded += 1;
    }
    assert_eq!(decoded, expected);
    assert_eq!(
        addr,
        data.len(),
        "instructions must tile the section exactly"
    );
}

/// Render an instruction the way the assembler's own syntax expects it, so that
/// the round trip source -> bytes covers the whole table.
fn render(ins: Instruction) -> String {
    use mcs51_isa::Operand as O;
    let mut parts = Vec::new();
    for op in ins.operands.iter() {
        parts.push(match *op {
            O::A => "A".to_string(),
            O::Ab => "AB".to_string(),
            O::C => "C".to_string(),
            O::Dptr => "DPTR".to_string(),
            O::AtDptr => "@DPTR".to_string(),
            O::AtADptr => "@A+DPTR".to_string(),
            O::AtAPc => "@A+PC".to_string(),
            O::Rn(n) => format!("R{n}"),
            O::AtRi(i) => format!("@R{i}"),
            O::Imm8(v) => format!("#0x{v:02X}"),
            O::Imm16(v) => format!("#0x{v:04X}"),
            O::Direct(d) => format!("0x{d:02X}"),
            O::Bit(b) => format!("0x{b:02X}"),
            O::NotBit(b) => format!("/0x{b:02X}"),
            // `$` is always in range for a relative branch and inside the same
            // 2 KiB page for AJMP/ACALL on a program this small.
            O::Rel(_) | O::Addr11(_) | O::Target(_) => "$".to_string(),
            O::Addr16(_) => "0x0000".to_string(),
        });
    }
    format!("        {} {}", ins.mnemonic.as_str(), parts.join(", "))
}

/// 字符串转义：`.db` 里必须支持 `\xNN`。
///
/// 这个 bug 曾经真实存在：生成器输出 `" s\x0D\x0A"`，而汇编器不认 `\xNN`，
/// 于是串口里打出来的不是换行，而是字面文本 `x0D`。
#[test]
fn string_escapes_cover_hex_bytes_and_the_usual_shorthands() {
    // 用 raw string：这里要的是**反斜杠序列本身**，不是 Rust 解出来的控制字符。
    let out = asm(r#"
        .area CODE (ABS)
        .org  0x0000
shorthand: .db  "a\r\n\tb", 0
hex:       .db  " \x0D\x0A", 0
mixed:     .db  "\x41\x42C", 0
solo:      .db  "\x9", 0
back:      .db  "C:\path", 0
"#);
    let data = &out.object.section("CODE").unwrap().data;
    let slice = |name: &str| -> Vec<u8> {
        let symbol = out.object.symbol(name).expect(name);
        let mcs51_asm::SymbolLoc::Section { offset, .. } = symbol.loc else {
            panic!("{name} is not a label");
        };
        let start = offset as usize;
        let end = data[start..]
            .iter()
            .position(|b| *b == 0)
            .expect("a NUL terminator")
            + start;
        data[start..end].to_vec()
    };
    assert_eq!(slice("shorthand"), b"a\r\n\tb");
    assert_eq!(slice("hex"), b" \r\n");
    assert_eq!(slice("mixed"), b"ABC");
    // 一位十六进制也要认（`\x9` 就是 0x09）。
    assert_eq!(slice("solo"), vec![0x09]);
    // 反斜杠后面不是已知转义时，原样保留。
    assert_eq!(slice("back"), b"C:\\path");
}

/// 芯片标准库里的名字（mcs51-chips）必须能直接在汇编里用。
///
/// 这些名字来自 Keil 的 `reg52.h` / `STC89C5xRC.h`：没有它们，写 8051 汇编
/// 就得记一堆十六进制地址，而记错一位是不会有任何报错的。
#[test]
fn the_chip_library_names_are_available_to_the_assembler() {
    let out = asm("
        .area CODE (ABS)
        .org  0x0000
start:
        MOV   TMOD, #0x01          ; reg52 里的字节寄存器
        MOV   TH0, #0x48
        MOV   TL0, #0x00
        SETB  TR0                  ; TCON 里的位
        JNB   TF0, start
        CLR   TF0
        MOV   SBUF, A
        SETB  EA
        SETB  P35                  ; Keil 风格的引脚名
        MOV   ISP_DATA, A          ; STC 扩展寄存器
        JB    P45, start           ; STC 的 P4 口（和 ALE 复用）
");
    assert!(out.is_ok(), "{}", out.render_diagnostics());
    let data = &out.object.section("CODE").unwrap().data;

    // MOV TMOD, #1 -> 75 89 01
    assert_eq!(&data[0..3], &[0x75, 0x89, 0x01]);
    // SETB TR0 -> D2 8C
    assert!(
        data.windows(2).any(|w| w == [0xD2, 0x8C]),
        "SETB TR0 没编出来"
    );
    // JNB TF0, rel -> 30 8D rel
    assert!(
        data.windows(2).any(|w| w == [0x30, 0x8D]),
        "JNB TF0 没编出来"
    );
    // SETB EA -> D2 AF
    assert!(
        data.windows(2).any(|w| w == [0xD2, 0xAF]),
        "SETB EA 没编出来"
    );
    // SETB P35 -> D2 B5
    assert!(
        data.windows(2).any(|w| w == [0xD2, 0xB5]),
        "SETB P35 没编出来"
    );
    // MOV ISP_DATA, A -> F5 E2
    assert!(
        data.windows(2).any(|w| w == [0xF5, 0xE2]),
        "MOV ISP_DATA,A 没编出来"
    );
    // JB P45, rel -> 20 ED rel
    assert!(
        data.windows(2).any(|w| w == [0x20, 0xED]),
        "JB P45 没编出来"
    );
}

/// 地址只能有一个来源：芯片标准库。
///
/// 这条测试是用**变异测试**逼出来的。最初汇编器把 `mcs51-isa` 的名字表放在
/// 芯片库前面，重名跳过 —— 于是 `TMOD`/`TR0`/`P3_5` 的地址实际上仍来自旧表，
/// 芯片库被完全遮蔽。把芯片库里的 `P35` 故意改错，生成的固件**毫无变化**，
/// 说明"接到芯片库上"只是表面功夫。
///
/// 修法是两层：芯片库排在最前面；下划线引脚名 `P3_5` 也**按名字**向芯片库要，
/// 而不是自己算 `端口基地址 + 序号`。
#[test]
fn every_hardware_symbol_resolves_through_the_chip_library() {
    let symbols = mcs51_asm::builtin_symbols();
    let lookup = |name: &str| -> Option<i64> {
        symbols
            .iter()
            .find(|(existing, _, _)| existing == name)
            .map(|(_, value, _)| *value)
    };

    // Keil 名字和下划线别名必须指向同一个地址 —— 它们只是同一个引脚的两套写法。
    for port in 0..4u8 {
        for bit in 0..8u8 {
            let keil = format!("P{port}{bit}");
            let underline = format!("P{port}_{bit}");
            let chip = mcs51_chips::reg52::CHIP
                .bit_named(&keil)
                .unwrap_or_else(|| panic!("芯片库缺 {keil}"));
            assert_eq!(
                lookup(&keil),
                Some(chip.addr as i64),
                "{keil} 的地址不是芯片库里的值"
            );
            assert_eq!(
                lookup(&underline),
                Some(chip.addr as i64),
                "{underline} 应该和芯片库的 {keil} 一致"
            );
        }
    }

    // 共用名字（芯片库和 mcs51-isa 都有的）也必须以芯片库为准。
    for sfr in mcs51_chips::reg52::SFR {
        assert_eq!(
            lookup(sfr.name),
            Some(sfr.addr as i64),
            "{} 的地址被别的表遮住了",
            sfr.name
        );
    }
    for bit in mcs51_chips::reg52::BITS {
        assert_eq!(
            lookup(bit.name),
            Some(bit.addr as i64),
            "{} 的地址被别的表遮住了",
            bit.name
        );
    }

    // STC 扩展也要在。
    assert_eq!(lookup("ISP_DATA"), Some(0xE2));
    assert_eq!(lookup("P45"), Some(0xED));
    assert_eq!(lookup("WDT_CONTR"), Some(0xE1));
}
