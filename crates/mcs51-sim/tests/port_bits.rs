//! 位操作对端口的影响。
//!
//! `MOV bit, C`（0x92）是软件 PWM 里最省事的一步：把借位直接搬进引脚，
//! 一条跳转都不用。这条测试盯住它不能影响端口上的其它位 ——
//! 尤其是 P3.0/P3.1，那是下载和串口用的脚。

use mcs51_asm::{Options, assemble};
use mcs51_ld::{LinkOptions, link};
use mcs51_sim::Machine;

/// 把一段代码装进固件跑起来，返回跑完之后的机器状态。
fn run(source: &str, steps: u64) -> Machine {
    let text = format!(
        "        .area CODE (ABS)\n        .org  0x0000\n        LJMP  start\n\
         \x20       .org  0x0030\nstart:\n{source}\n        SJMP  $\n"
    );
    let output = assemble(&text, &Options::new("bits.asm"));
    assert!(output.is_ok(), "{}", output.render_diagnostics());
    let linked = link(&[output.object], &LinkOptions::default());
    assert!(linked.is_ok(), "{}", linked.render_diagnostics());
    let mut machine = Machine::from_image(&linked.code);
    machine.halt_on_self_jump = false;
    for _ in 0..steps {
        machine.step();
    }
    machine
}

#[test]
fn mov_bit_c_only_touches_that_bit() {
    // P3 复位是 0xFF。清 C 后把 C 搬进 P3.7，应该只有 bit7 变 0。
    let machine = run("        CLR   C\n        MOV   P3_7, C\n", 100);
    let p3 = machine.peripherals().latch(3);
    assert_eq!(
        p3, 0x7F,
        "MOV P3_7, C 之后 P3 应该是 0x7F（只有 bit7 变 0），实际 {p3:#04X}"
    );
}

#[test]
fn setb_and_clr_only_touch_their_bit() {
    let machine = run("        CLR   P3_5\n        SETB  P3_6\n", 100);
    let p3 = machine.peripherals().latch(3);
    assert_eq!(
        p3, 0xDF,
        "CLR P3_5 + SETB P3_6 之后 P3 应该是 0xDF，实际 {p3:#04X}"
    );
}

#[test]
fn a_sequence_of_bit_writes_preserves_the_others() {
    // 模拟 PWM 驱动：反复把不同的借位搬进 P3.7，P3.5/P3.6 必须纹丝不动。
    let machine = run(
        "        CLR   P3_5\n        CLR   P3_6\n\
         \x20       CLR   C\n        MOV   P3_7, C\n\
         \x20       SETB  C\n        MOV   P3_7, C\n\
         \x20       CLR   C\n        MOV   P3_7, C\n",
        200,
    );
    let p3 = machine.peripherals().latch(3);
    // bit7=0（最后清掉）、bit6=0、bit5=0，其余为 1 → 0b0001_1111
    assert_eq!(
        p3, 0x1F,
        "P3.5/P3.6/P3.7 都为 0、其余为 1 → 0x1F，实际 {p3:#04X}"
    );
}

#[test]
fn the_simulator_reports_the_whole_port_in_the_high_bits() {
    // 回归：PCON(0x87) 曾经和 P0 的锁存混在一起，导致进空闲模式时端口乱掉。
    // 这里确认 P3 的高 5 位在只动低位时保持为 1。
    let machine = run("        CLR   P3_7\n", 50);
    let p3 = machine.peripherals().latch(3);
    // 0x7F：只有 bit7 变 0，bit6..bit0 仍是 1。
    assert_eq!(p3, 0x7F, "只动 bit7，其余位不该变：{p3:#04X}");
}
