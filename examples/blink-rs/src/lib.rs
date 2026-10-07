//! # 裸机 blink + 串口报数 —— 应用逻辑
//!
//! 这是 [`examples/blink`](../../blink/)（152 字节汇编版）的 Rust 版，
//! **同一个程序**。整个外设配置只有三行：
//!
//! ```text
//! let console = Console::install(&mut app, CPU_HZ, 9600)   // 串口 + 打印子程序
//!     .with_hex(&mut app);                              // 再加数字打印
//! let led = Gpio::active_low(P10);                          // 引脚 + 极性
//! let tick = Timer::timer0().every_ms(CPU_HZ, 500);          // 周期 500ms
//! ```
//!
//! 逻辑放在 `lib.rs` 而不是 `main.rs`，是为了让 `tests/` 能直接调用
//! [`build`] 拿到 `Firmware` 丢进模拟器，不用去解析产物文件。
//!
//! ```bash
//! cargo run
//! mcs51 build examples/blink-rs
//! mcs51 run   examples/blink-rs
//! ```

use mcs51_chips::stc89c5xrc::*;
use mcs51_codegen::prelude::*;

/// 板子上的晶振。11.0592MHz 是 8051 的标准值：它让 9600 波特率精确整除。
pub const CPU_HZ: u32 = 11_059_200;

/// 翻转周期，毫秒。
///
/// 16 位定时器在 11.0592MHz 下最多数约 71ms，`every_ms` 会自动把它拆成
/// 8 次溢出（每次 57584 个机器周期），总时长正好 500ms。
pub const PERIOD_MS: u32 = 500;

/// 串口波特率。
pub const BAUD: u32 = 9600;

/// 组装固件。
pub fn build(name: &str) -> Firmware {
    let mut app = App::new(
        Board::default()
            .named(name)
            .device(CHIP.name)
            .crystal(CPU_HZ)
            .baud(BAUD),
    );

    // ---- 外设：三行配完 ----
    let console = Console::install(&mut app, CPU_HZ, BAUD).with_hex(&mut app);
    let led = Gpio::active_low(P10);
    let tick = Timer::timer0().every_ms(CPU_HZ, PERIOD_MS);
    let counter = app.statics(1);

    app.main(move |t| {
        console.begin(t); // Timer 1 模式 2 + TH1 + SCON + PCON + TR1
        tick.begin(t); // 读-改-写 TMOD + 装初值 + SETB TR0
        t.mov_direct_imm(counter, 0);

        t.forever(|t| {
            tick.wait(t); // 等 500ms（轮询 TF0 + 清标志 + 重装，一次做完）
            led.toggle(t);

            console.print(t, "t=");
            t.mov_a_direct(counter);
            console.print_hex8(t);
            console.newline(t);
            t.inc_direct(counter);
        });
    });

    app.finish().expect("生成裸机固件失败")
}

/// 产物名：`mcs51 build` 通过环境变量传入，直接 `cargo run` 时用内置名字。
pub fn project_name(fallback: &str) -> String {
    std::env::var("MCS51_PROJECT_NAME").unwrap_or_else(|_| fallback.to_string())
}

/// 把固件写进 `build/`（或 `MCS51_OUT_DIR`）并打印尺寸报告。
pub fn emit(firmware: Firmware) -> Result<(), String> {
    let out = match std::env::var("MCS51_OUT_DIR") {
        Ok(dir) => std::path::PathBuf::from(dir),
        Err(_) => std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("build"),
    };
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let files = firmware
        .write_to(&out)
        .map_err(|e| format!("写入产物失败: {e}"))?;
    println!("生成完成：{}", firmware.board.name);
    for path in files {
        println!("  {}", path.strip_prefix(root).unwrap_or(&path).display());
    }
    println!();
    print!("{}", firmware.size_report());
    Ok(())
}
