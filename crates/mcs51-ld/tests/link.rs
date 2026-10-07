//! Linker tests: placement, symbol resolution, relocation and diagnostics.

use mcs51_asm::{Options, assemble};
use mcs51_isa::mem::Space;
use mcs51_ld::{LinkOptions, LinkerScript, link};

fn object(name: &str, source: &str) -> mcs51_asm::ObjectFile {
    let out = assemble(source, &Options::new(name));
    assert!(
        out.is_ok(),
        "assembly failed for {name}:\n{}",
        out.render_diagnostics()
    );
    out.object
}

fn link_ok(objects: &[mcs51_asm::ObjectFile]) -> mcs51_ld::LinkOutput {
    let out = link(objects, &LinkOptions::default());
    assert!(out.is_ok(), "link failed:\n{}", out.render_diagnostics());
    out
}

#[test]
fn places_home_and_code_and_wires_the_vectors() {
    let main = object(
        "main.asm",
        "
        .include \"8051.inc\"
        .area HOME (ABS)
        .org  0x0000
        LJMP  start
        .org  0x000B
        LJMP  tick
        .area CODE
start:  MOV   SP, #0x30
        SJMP  start
tick:   RETI
        ",
    );
    let out = link_ok(&[main]);

    assert_eq!(out.entry, None);
    assert_eq!(out.code.get(0x0000), Some(0x02));
    assert_eq!(out.code.get(0x0001), Some(0x00));
    assert_eq!(out.code.get(0x000B), Some(0x02));

    // `start` is the first byte of the CODE section, right after the 0x0E bytes
    // of pinned interrupt vectors in HOME.
    let start = out.symbols.get("start").expect("start");
    assert_eq!(start.value, 0x000E);
    assert_eq!(out.code.get(0x000E), Some(0x75)); // MOV SP,#0x30
    assert_eq!(out.code.get(0x000F), Some(0x81));
    assert_eq!(out.code.get(0x0010), Some(0x30));
    let tick = out.symbols.get("tick").expect("tick");
    assert_eq!(tick.value, 0x0013);
}

#[test]
fn resolves_symbols_between_objects() {
    let lib = object(
        "lib.asm",
        "
        .area CODE
        .globl delay
delay:  MOV   R7, #10
.loop:  DJNZ  R7, .loop
        RET
        ",
    );
    let app = object(
        "app.asm",
        "
        .include \"8051.inc\"
        .area HOME (ABS)
        .org  0x0000
        LJMP  main
        .area CODE
        .globl main
main:   LCALL delay
        SJMP  main
        ",
    );
    let out = link_ok(&[lib, app]);
    let delay = out.symbols.get("delay").expect("delay").value;
    let main = out.symbols.get("main").expect("main").value;
    // HOME (the 3-byte vector jump in app.asm) is placed first, then the CODE
    // contributions in object order.
    assert_eq!(delay, 0x0003, "delay should follow the vector jump");
    assert!(main > delay);

    let lcall = main as u16;
    assert_eq!(out.code.get(lcall), Some(0x12));
    let target =
        ((out.code.get(lcall + 1).unwrap() as u32) << 8) | out.code.get(lcall + 2).unwrap() as u32;
    assert_eq!(target, delay);
}

#[test]
fn resolves_absolute_bit_and_immediate_relocations() {
    let lib = object(
        "lib.asm",
        "
        .area BIT (ABS)
        .org  0x20
flag    .bit  0x20
        .area DATA (ABS)
        .org  0x30
counter .ds   1
        .area CODE
        .globl flag, counter
        .globl message
message: .db \"hi\", 0
        ",
    );
    let app = object(
        "app.asm",
        "
        .area CODE
        .extern flag, counter, message
        .globl start
start:  SETB  flag
        MOV   counter, #1
        MOV   DPTR, #message
        MOV   A, #low(message)
        MOV   A, #high(message)
        ",
    );
    let out = link_ok(&[lib, app]);
    assert_eq!(out.symbols.get("flag").unwrap().value, 0x20);
    assert_eq!(out.symbols.get("counter").unwrap().value, 0x30);
    let message = out.symbols.get("message").unwrap().value;
    let start = out.symbols.get("start").unwrap().value as u16;

    let bytes: Vec<u8> = (0..12).map(|i| out.code.get(start + i).unwrap()).collect();
    assert_eq!(bytes[0], 0xD2); // SETB bit
    assert_eq!(bytes[1], 0x20);
    assert_eq!(&bytes[2..5], &[0x75, 0x30, 0x01]); // MOV direct,#imm
    assert_eq!(&bytes[5..8], &[0x90, (message >> 8) as u8, message as u8]);
    assert_eq!(&bytes[8..10], &[0x74, (message & 0xFF) as u8]);
    assert_eq!(&bytes[10..12], &[0x74, ((message >> 8) & 0xFF) as u8]);
}

#[test]
fn reports_undefined_symbols() {
    let app = object(
        "app.asm",
        "
        .area CODE
start:  LCALL nowhere
        ",
    );
    let out = link(&[app], &LinkOptions::default());
    assert!(!out.is_ok());
    assert!(
        out.render_diagnostics()
            .contains("undefined symbol `nowhere`")
    );
}

#[test]
fn reports_region_overflow() {
    let objects = [
        object(
            "a.asm",
            "
            .area CODE (ABS)
            .org 0x7FF0
            .ds  15
            ",
        ),
        object(
            "b.asm",
            "
            .area CODE
big:    .ds  64
            ",
        ),
    ];
    let out = link(&objects, &LinkOptions::default());
    assert!(!out.is_ok());
    let text = out.render_diagnostics();
    assert!(
        text.contains("overflows region") || text.contains("does not fit"),
        "{text}"
    );
}

#[test]
fn honours_a_custom_linker_script() {
    let app = object(
        "app.asm",
        "
        .area CODE
main:   NOP
        SJMP main
        ",
    );
    let script = LinkerScript::parse(
        "\
memory FLASH code 0x0200 0x0100
section CODE -> FLASH
entry main
",
    )
    .unwrap();
    let out = link(
        &[app],
        &LinkOptions {
            script,
            ..Default::default()
        },
    );
    assert!(out.is_ok(), "{}", out.render_diagnostics());
    assert_eq!(out.symbols.get("main").unwrap().value, 0x0200);
    assert_eq!(out.entry, Some(0x0200));
    assert_eq!(out.code.get(0x0200), Some(0x00));
    assert_eq!(out.code.get(0x0201), Some(0x80));
    assert_eq!(out.code.get(0x0202), Some(0xFD));
}

#[test]
fn writes_a_checksum_byte() {
    let app = object(
        "app.asm",
        "
        .area CODE (ABS)
        .org 0x0000
        .db  1, 2, 3
        .org 0x0010
        .db  0
        ",
    );
    let script = LinkerScript::parse(
        "\
memory CODE 0x0000 0x8000
section CODE -> CODE
checksum 0x0010
",
    )
    .unwrap();
    let out = link(
        &[app],
        &LinkOptions {
            script,
            ..Default::default()
        },
    );
    assert!(out.is_ok(), "{}", out.render_diagnostics());
    let total: u32 = out.code.iter().map(|(_, b)| b as u32).sum();
    assert_eq!(total % 256, 0, "checksum must make the image sum to zero");
}

#[test]
fn map_file_mentions_sections_and_symbols() {
    let app = object(
        "app.asm",
        "
        .area HOME (ABS)
        .org 0x0000
        LJMP main
        .area CODE
        .globl main
main:   NOP
        ",
    );
    let out = link_ok(&[app]);
    let map = out.map_file(&[]);
    assert!(map.contains("Memory regions"));
    assert!(map.contains("Sections"));
    assert!(map.contains("main"));
    assert!(map.contains("CODE"));
    assert_eq!(out.sections.len(), 2);
    assert!(out.data.is_empty(), "no data sections in this program");
}

#[test]
fn data_sections_land_in_their_own_space() {
    let app = object(
        "app.asm",
        "
        .area CODE
main:   NOP
        .area XDATA (ABS)
        .org 0x1000
buffer  .ds 16
        .area DATA (ABS)
        .org 0x40
count   .ds 1
        ",
    );
    let out = link_ok(&[app]);
    assert_eq!(out.symbols.get("buffer").unwrap().value, 0x1000);
    assert_eq!(out.symbols.get("count").unwrap().value, 0x40);
    assert!(out.data.contains_key(&Space::Xdata));
    assert!(out.data.contains_key(&Space::Data));
    // The code image only holds program memory.
    assert_eq!(out.code.len(), 1);
}

#[test]
fn ajmp_across_objects_is_page_checked() {
    let far = object(
        "far.asm",
        "
        .area CODE (ABS)
        .org 0x0900
        .globl far
far:    RET
        ",
    );
    let near = object(
        "near.asm",
        "
        .area CODE (ABS)
        .org 0x0000
        .globl near
near:   AJMP far
        ",
    );
    let out = link(&[near, far], &LinkOptions::default());
    assert!(!out.is_ok());
    assert!(
        out.render_diagnostics().contains("2 KiB page"),
        "{}",
        out.render_diagnostics()
    );
}
