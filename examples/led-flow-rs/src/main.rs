//! # 流水灯 + 呼吸灯（Rust 裸机版）—— 应用入口
//!
//! * **P3.5 / P3.6**：轮流点亮的流水灯，每只 250ms；
//! * **P3.7**：用 **Timer 0** 做软件 PWM 的呼吸灯。
//!
//! 两者共用**同一个 Timer 0**：
//!
//! ```text
//!   Timer 0 每 0.2ms 溢出一次  ─┬─ 每个 tick : 推进 PWM 相位，驱动 P3.7
//!                               ├─ 每 50 个 tick（10ms）：占空比沿斜坡推一格
//!                               └─ 每 1200 个 tick（250ms）：换一只流水灯
//! ```
//!
//! 裸机版**没有用中断**：主循环轮询 `TF0`。定时器配置全部交给 [`Timer`] ——
//! 周期写成"多少微秒"，重载值在主机上算好，`TMOD` 用读-改-写免得抹掉
//! 另一个定时器的配置。
//!
//! ```bash
//! cargo run     # 生成固件到 build/
//! ```
//!
//! ## 为什么这不是「Rust 跑在 8051 上」
//!
//! `rustc` 依赖 LLVM，而 LLVM 没有 MCS-51 后端。所以这个程序在**主机上运行**，
//! 在构建期生成 8051 机器码。但**工程结构是真的**：普通 Rust 模块、类型检查、
//! `cargo test`、跨模块调用、目标机子程序复用。

mod board;
mod breath;
mod leds;

use mcs51_codegen::prelude::*;

use crate::board::{CPU_HZ, TICK_MICROS, TICK_OVERHEAD};

fn main() {
    let firmware = bare();
    if let Err(error) = emit(firmware) {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

/// 裸机版：一个 Timer 0 时基驱动流水灯和呼吸灯。
pub fn bare() -> Firmware {
    let mut app = App::new(board::board(&project_name("led-flow-rs")));

    // 静态变量：呼吸灯要 4 个字节，流水灯要 2 个。
    // 裸机模式下从 0x20 往上放（0x30 是栈底，栈向上增长，碰不到）。
    let breath = breath::Breath::new(app.statics(4));
    let chaser = leds::Chaser::new(app.statics(2));

    // 定时器：只说"每 200 微秒溢出一次"，重载值在主机上算。
    // compensate() 扣掉"主循环发现溢出并重装"那十几条指令占用的时间。
    let tick = Timer::timer0()
        .micros(CPU_HZ, TICK_MICROS)
        .compensate(TICK_OVERHEAD);

    app.main(move |t| {
        chaser.init(t);
        breath.init(t);
        tick.begin(t); // 读-改-写 TMOD + 装初值 + 启动

        let spin = t.new_label("spin");
        let wrapped = t.new_label("wrapped");
        let after = t.new_label("after_period");

        t.forever(|t| {
            t.comment("等 Timer 0 溢出（裸机不用中断，所以轮询 TF0）");
            t.label(spin);
            t.jnb(tick.overflow_flag(), spin);

            tick.clear_flag(t);
            tick.reload(t);

            breath.advance(t, wrapped);
            t.sjmp(after);

            // ---- 一个 PWM 周期结束（10ms）----
            t.label(wrapped);
            breath.ramp(t);
            chaser.tick(t);

            t.label(after);
            breath.drive(t);
        });
    });

    app.finish().expect("生成裸机固件失败")
}

/// 产物名：`mcs51 build` 通过环境变量传入，直接 `cargo run` 时用内置名字。
fn project_name(fallback: &str) -> String {
    std::env::var("MCS51_PROJECT_NAME").unwrap_or_else(|_| fallback.to_string())
}

fn emit(firmware: Firmware) -> Result<(), String> {
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
