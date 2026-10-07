//! 行为测试：把生成的固件真的放进模拟器跑，断言它做了什么。
//!
//! 这些断言盯着的是**外设配置的正确性**，不是实现细节：
//! 波特率算对没有、`TMOD` 有没有把 Timer 1 的位碰坏、LED 到底闪没闪。

use mcs51_sim::{Machine, StopReason};

fn firmware() -> mcs51_codegen::Firmware {
    blink_rs::build("blink-rs-test")
}

#[test]
fn the_uart_is_configured_for_exactly_9600_baud() {
    // 11.0592MHz 是 8051 的标准晶振，正因为 9600 能精确整除。
    // HAL 要在主机上把这笔账算准，而不是让每个应用自己推 TH1。
    let asm = firmware().assembly;
    assert!(asm.contains("MOV   TH1, #253"), "TH1 应该是 0xFD：\n{asm}");
    assert!(asm.contains("MOV   TL1, #253"), "TL1 应该是 0xFD");
    assert!(
        asm.contains("MOV   SCON, #80"),
        "SCON 应该是 0x50（模式 1 + REN）"
    );
    assert!(asm.contains("SETB  TR1"), "要启动波特率发生器");
}

#[test]
fn timer0_is_set_up_with_read_modify_write() {
    // Timer 1 正供着串口波特率，整字节写 TMOD 会把它打死。
    // 这是手写代码最容易犯的错，HAL 必须替用户避开。
    let asm = firmware().assembly;
    assert!(asm.contains("MOV   A, TMOD"), "TMOD 必须读-改-写：\n{asm}");
    assert!(asm.contains("ANL   A, #240"), "先保留 Timer 1 的位");
    assert!(asm.contains("ORL   A, #1"), "再把 Timer 0 设成模式 1");
    assert!(!asm.contains("MOV   TMOD, #"), "不该整字节覆盖 TMOD");
}

#[test]
fn the_tick_period_is_really_500_ms_not_71_ms() {
    // 回归测试：`Timer::millis` 曾经把 500ms 静默钳位到 71ms（16 位定时器的量程），
    // 模拟器里"有输出"但周期全错。所以必须断言**机器码里的重载值**。
    let asm = firmware().assembly;
    // 500ms @ 11.0592MHz = 460800 个机器周期。
    // 一次装不下（16 位最多 65536），所以拆成 8 次溢出，每次数 57600 个周期，
    // 扣掉 16 个周期的软件开销 → 装 57584。
    // 重载值 = 65536 - 57584 = 7952 = 0x1F10 → TH0 = 0x1F, TL0 = 0x10。
    assert!(
        asm.contains("MOV   TH0, #31") && asm.contains("MOV   TL0, #16"),
        "TH0/TL0 应该是 0x1F10（重载值 7952 = 65536 - 57584）：\n{asm}"
    );
    // 钳位成 71ms 的话重载值会是 0x0010，也就是 TH0 = 0 ——
    // 上面那条断言里的 TH0 = 31 正是用来区分这两种情况的。
    // 而且等待必须真的展开 8 次。
    let waits = asm.matches("CLR   TF0").count();
    assert_eq!(waits, 8, "应该等 8 次溢出，实际 {waits} 次");
}

#[test]
fn it_toggles_the_led_and_prints_a_count_half_a_second_apart() {
    let fw = firmware();
    let mut machine = Machine::from_image(&fw.image);
    machine.halt_on_self_jump = false;
    machine.peripherals_mut().log_ports = true;
    machine.run_cycles(6_000_000); // 大约 13 个 500ms 周期

    // P1.0 真的在翻转，而且**别翻得太快** —— 7Hz 和 1Hz 必须能区分开。
    // 6M 个机器周期 / 500ms(460800) ≈ 13 次翻转；钳位成 71ms 的话会是 84 次。
    let p1: Vec<u8> = machine
        .peripherals()
        .port_log
        .iter()
        .filter(|(address, _)| *address == 0x90)
        .map(|(_, value)| *value)
        .collect();
    let toggles = (1..p1.len())
        .filter(|i| (p1[*i - 1] ^ p1[*i]) & 0x01 != 0)
        .count();
    assert!(
        toggles >= 3,
        "P1.0 应该反复翻转，实际 {toggles} 次：{p1:02X?}"
    );

    // 串口上真的打出了 "t=NN"，而且**数值在递增**。
    let console = machine.console_string();
    let values: Vec<&str> = console
        .lines()
        .filter_map(|line| line.strip_prefix("t="))
        .collect();
    assert!(values.len() >= 3, "应该打出不止一行：{console:?}");
    for (index, value) in values.iter().enumerate() {
        assert_eq!(
            *value,
            format!("{index:02X}"),
            "第 {index} 行应该是 {index:02X}，实际 {value:?}\n{console}"
        );
    }
}

#[test]
fn it_keeps_running_instead_of_jumping_into_the_weeds() {
    // `run_cycles` 用"跑够周期数"当谓词，所以正常结束时返回 `Predicate`。
    // 真的跑到别处去的话会先撞上 `Halted`（PC 落到 0xFF 或自跳转）。
    let fw = firmware();
    let mut machine = Machine::from_image(&fw.image);
    machine.halt_on_self_jump = false;
    let budget = 4_000_000u64;
    let reason = machine.run_cycles(budget);
    assert_ne!(reason, StopReason::Halted, "固件中途停了");
    assert!(
        machine.snapshot().cycles >= budget,
        "只跑了 {} 个周期，没跑满预算",
        machine.snapshot().cycles
    );
    // PC 必须还落在代码里（代码只有两百来字节）。
    let code_bytes = fw.code_size() as u16;
    let pc = machine.snapshot().pc;
    assert!(
        pc < code_bytes,
        "PC 0x{pc:04X} 跑到代码区（{code_bytes} 字节）外面了"
    );
}
