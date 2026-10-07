//! 行为测试：跑**真实产物**，不是内存里的中间态。
//!
//! 测试先执行 `src/main.rs` 生成固件，再读回它写出的 `.ihx`，
//! 装进模拟器断言行为。这样测到的就是 `mcs51 flash` 会烧进芯片的那份东西。

use std::path::PathBuf;
use std::process::Command;

use mcs51_sim::Machine;

/// 跑生成器，产出固件，然后把它读进模拟器。
fn firmware() -> Machine {
    let out = generate();
    let hex = std::fs::read_to_string(out.join("behaviour-test.ihx")).expect("读产物");
    let image = mcs51_ihex::Image::from_intel_hex(&hex).expect("产物应该是合法的 Intel HEX");
    Machine::from_image(&image)
}

fn generate() -> PathBuf {
    let out = unique_dir();
    std::fs::create_dir_all(&out).expect("建临时目录");
    let status = Command::new(env!("CARGO_BIN_EXE_led-flow-rs"))
        .env("MCS51_OUT_DIR", &out)
        .env("MCS51_PROJECT_NAME", "behaviour-test")
        .status()
        .expect("生成器应该能启动");
    assert!(status.success(), "生成器失败了");
    out
}

/// 每次调用给一个独立目录 —— 测试并行跑，共用目录会互相删产物。
fn unique_dir() -> PathBuf {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    std::env::temp_dir().join(format!(
        "mcs51-behaviour-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ))
}

/// 逐周期采样 P3，得到 (周期, P3 值) 序列。
fn trace_port(mut machine: Machine, port: usize, cycles: u64) -> Vec<(u64, u8)> {
    let mut events = Vec::new();
    let mut last = machine.peripherals().latch(port);
    while machine.cycles < cycles {
        machine.step();
        let value = machine.peripherals().latch(port);
        if value != last {
            events.push((machine.cycles, value));
            last = value;
        }
    }
    events
}

/// P3.7 是否处于点亮（低电平）。
fn breathing_on(value: u8) -> bool {
    value & (1 << 7) == 0
}

/// 哪只流水灯亮着（P3.5/P3.6）。
fn chasing(value: u8) -> Option<u8> {
    (5..=6).find(|bit| value & (1 << bit) == 0)
}

#[test]
fn the_chaser_alternates_every_quarter_second() {
    let mut machine = firmware();
    machine.halt_on_self_jump = false;
    let events = trace_port(machine, 3, 1_400_000);

    // 换灯的时刻：哪只灯亮的序列必须是 5,6,5,6...
    let mut switches = Vec::new();
    let mut last = None;
    for (cycle, value) in &events {
        if let Some(bit) = chasing(*value) {
            if last != Some(bit) {
                switches.push((*cycle, bit));
                last = Some(bit);
            }
        }
    }
    assert!(switches.len() >= 4, "至少要看两次换灯，实际 {switches:?}");
    let order: Vec<u8> = switches.iter().map(|(_, bit)| *bit).collect();
    assert_eq!(&order[..4], &[5, 6, 5, 6], "流水顺序不对：{order:?}");

    // 每只灯亮 250ms @ 11.0592MHz ≈ 230400 个机器周期。
    for pair in switches.windows(2) {
        let gap = pair[1].0 - pair[0].0;
        assert!(
            (215_000..=245_000).contains(&gap),
            "换灯间隔是 {gap} 个周期，期望约 230400（250ms）"
        );
    }
}

#[test]
fn the_breathing_led_ramps_up_and_down() {
    // 核心断言：P3.7 的占空比要**从暗升到亮，再降回暗**。
    //
    // 不断言具体的呼吸周期（那取决于 RAMP_HOLD / PWM_PERIOD，属于参数而不是
    // 行为），只断言形状：起点暗 → 中间亮 → 回到暗，而且真的在呼吸（不是常亮）。
    const WINDOW: u64 = 9_216; // 10ms @ 11.0592MHz，正好一个 PWM 周期
    const WINDOWS: usize = 1400; // 14 秒，足够容纳一个完整呼吸周期

    let mut machine = firmware();
    machine.halt_on_self_jump = false;
    let initial = machine.peripherals().latch(3);
    let events = trace_port(machine, 3, WINDOW * WINDOWS as u64);

    // 把 P3.7 的点亮时间累加到每个 10ms 窗口里。
    let mut lit = vec![0u64; WINDOWS];
    // 复位后端口是哪一态不该影响结论，所以先跳过前 20 个窗口当热身。
    let warmup = 20;
    let mut level = breathing_on(initial);
    let mut prev = 0u64;
    for (cycle, value) in &events {
        if level {
            let mut cursor = prev;
            while cursor < *cycle {
                let window = (cursor / WINDOW) as usize;
                if window >= WINDOWS {
                    break;
                }
                let end = ((window as u64 + 1) * WINDOW).min(*cycle);
                lit[window] += end - cursor;
                cursor = end;
            }
        }
        level = breathing_on(*value);
        prev = *cycle;
    }
    let duty: Vec<f64> = lit
        .iter()
        .enumerate()
        .map(|(i, n)| {
            if i < warmup {
                0.0
            } else {
                *n as f64 / WINDOW as f64
            }
        })
        .collect();

    let brightest = duty[warmup..].iter().cloned().fold(f64::MIN, f64::max);
    let darkest = duty[warmup..].iter().cloned().fold(f64::MAX, f64::min);
    let peak = duty
        .iter()
        .position(|d| *d >= brightest * 0.95)
        .expect("总该有个峰值");

    assert!(
        darkest < 0.15,
        "最暗时占空比还有 {darkest:.2}，应该接近熄灭"
    );
    assert!(
        brightest > 0.85,
        "最亮时占空比只有 {brightest:.2}，应该接近全亮"
    );

    // 峰值不能贴着两端 —— 那样只看到单调的上升或下降，不叫呼吸。
    let margin = WINDOWS / 10;
    assert!(
        (margin..WINDOWS - margin).contains(&peak),
        "最亮的一刻在第 {peak} 个窗口，不该贴在两端（{WINDOWS} 个窗口）"
    );
    assert!(
        duty[warmup] < 0.3 || darkest < 0.15,
        "热身后应该是暗的，实际 {:.2}",
        duty[warmup]
    );

    // 占空比必须真的在变 —— 常亮或常灭都不叫呼吸。
    let spread = brightest - darkest;
    assert!(
        spread > 0.7,
        "占空比只从 {darkest:.2} 变到 {brightest:.2}，没有呼吸"
    );

    // 峰值两侧都要能看到明显的上升和下降（不是一路涨到底）。
    let rising = duty[warmup..peak].iter().cloned().fold(f64::MIN, f64::max);
    let falling = duty[peak..].iter().cloned().fold(f64::MIN, f64::max);
    assert!(rising > 0.3, "峰值之前没涨起来");
    assert!(falling > 0.3, "峰值之后没看出来（采样窗口太短？）");
}

#[test]
fn the_two_leds_never_interfere() {
    // 流水灯用 P3.5/P3.6，呼吸灯用 P3.7 —— 各自独立。
    let mut machine = firmware();
    machine.halt_on_self_jump = false;
    for (cycle, value) in trace_port(machine, 3, 1_000_000) {
        let chasing_count = (5..=6).filter(|bit| value & (1 << bit) == 0).count();
        assert!(
            chasing_count <= 1,
            "周期 {cycle} 时有 {chasing_count} 只流水灯同时亮（P3={value:#04X}）"
        );
    }
}

#[test]
fn the_serial_pins_are_never_pulled_low() {
    // P3.0/P3.1 是下载和串口用的脚，被拉低会让下一次下载失败。
    let mut machine = firmware();
    machine.halt_on_self_jump = false;
    for (cycle, value) in trace_port(machine, 3, 800_000) {
        assert_eq!(
            value & 0x03,
            0x03,
            "周期 {cycle} 时 P3={value:#04X}，串口脚被拉低了"
        );
    }
}

#[test]
fn the_bare_firmware_is_small_and_uses_timer0_without_interrupts() {
    let out = generate();
    let asm = std::fs::read_to_string(out.join("behaviour-test.asm")).expect("读生成结果");

    // 裸机版的意义就是「不要内核、不要中断」。
    for symbol in ["os_task_table", "os_sleep", "os_uart_puts"] {
        assert!(!asm.contains(symbol), "裸机版不该引用内核符号 `{symbol}`");
    }
    // Timer 0 必须真的被用起来，而且 TMOD 必须是**读-改-写**：
    // 整字节写会把 Timer 1 的配置（比如串口波特率）一起抹掉。
    assert!(
        asm.contains("MOV   A, TMOD"),
        "TMOD 应该读-改-写，而不是整字节覆盖：\n{asm}"
    );
    assert!(
        asm.contains("ANL   A, #240"),
        "读-改-写要先保留 Timer 1 的位"
    );
    assert!(asm.contains("ORL   A, #1"), "再把 Timer 0 设成模式 1");
    assert!(!asm.contains("MOV   TMOD, #"), "不该整字节覆盖 TMOD");
    assert!(asm.contains("SETB  TR0"), "应该启动 Timer 0");
    assert!(asm.contains("JNB   TF0"), "裸机版靠轮询 TF0");
    // 中断向量表里 Timer 0 那条仍然是 RETI —— 没有装中断处理程序。
    assert!(!asm.contains("os_timer0_isr"));

    let bin = std::fs::read(out.join("behaviour-test.bin")).expect("读二进制");
    assert!(bin.len() < 256, "裸机版应该很小，实际 {} 字节", bin.len());
}
