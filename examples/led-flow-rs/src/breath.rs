//! 呼吸灯模块：用 Timer 0 做软件 PWM，驱动 P3.7。
//!
//! ## 原理
//!
//! LED 的亮度靠**占空比**：一个 PWM 周期里点亮的时间比例。8051 没有硬件 PWM，
//! 所以用 Timer 0 定时打断，每次中断推进一步：
//!
//! ```text
//!   一个 PWM 周期 = PWM_PERIOD 个 tick（50 × 0.2ms = 10ms = 100Hz）
//!
//!   tick:  0  1  2 ... 24 25 ... 49 | 0  1 ...
//!   phase: 0  1  2 ... 24 25 ... 49 | 0  1 ...
//!   duty:  25                       |  26      ← 每 RAMP_HOLD 个周期变一格
//!   LED:   ████████████░░░░░░░░░░░░  | █████████████░░░░░░░░░░░
//!          └─ phase < duty 就点亮 ─┘
//! ```
//!
//! `duty` 从 0 慢慢涨到 PWM_PERIOD，再慢慢退回 0，就是一次呼吸。
//!
//! ## 省掉一次比较的技巧
//!
//! 判断 `phase < duty` 不用真做减法再比：`SUBB A, duty` 之后**借位 C 就是答案**
//! （`phase < duty` 时 C = 1）。而 8051 有 `MOV bit, C`，可以把借位直接搬到引脚上，
//! 一条跳转都不用。

use mcs51_codegen::prelude::*;

use crate::board::{BREATH_PIN, PWM_PERIOD, RAMP_HOLD};

/// 呼吸灯的目标机状态。
///
/// 字段里存的是**目标机内部 RAM 的地址**，不是主机上的值 —— eDSL 里的
/// "驱动对象"都是这么写的：Rust 侧的对象只是把地址和常量捆在一起，
/// 方法负责生成操作这些地址的代码。
#[derive(Debug, Clone, Copy)]
pub struct Breath {
    /// 当前 PWM 相位，0..PWM_PERIOD-1。
    phase: u8,
    /// 当前占空比，0..=PWM_PERIOD。
    duty: u8,
    /// 斜坡方向：0 = 变亮，1 = 变暗。
    dir: u8,
    /// 还有几个 PWM 周期换下一格占空比。
    hold: u8,
}

impl Breath {
    /// `base` 是 [`App::statics`] 分配的 4 个字节。
    pub fn new(base: u8) -> Self {
        Self {
            phase: base,
            duty: base + 1,
            dir: base + 2,
            hold: base + 3,
        }
    }

    /// 初始化：从最暗开始，方向朝变亮。
    pub fn init(&self, t: &mut Task) {
        t.comment("呼吸灯：从最暗开始，先变亮");
        t.mov_direct_imm(self.phase, 0);
        t.mov_direct_imm(self.duty, 0);
        t.mov_direct_imm(self.dir, 0);
        t.mov_direct_imm(self.hold, RAMP_HOLD);
    }

    /// 每个 tick 调一次：推进相位，回绕时跳到 `wrapped`。
    ///
    /// 调用方在 `wrapped` 处放"一个 PWM 周期结束了"的代码（推斜坡、换流水灯），
    /// 没回绕就继续往下走（通常是跳过那段）。
    ///
    /// 新相位会存在 `phase` 里，所以调用方两边汇合后直接调 [`Breath::drive`]。
    pub fn advance(&self, t: &mut Task, wrapped: Label) {
        let keep = t.new_label("phase_keep");
        t.comment("推进 PWM 相位，到 PWM_PERIOD 就回绕，并告诉调用方一个周期结束了");
        t.mov_a_direct(self.phase);
        t.inc_a();
        t.cjne_a_imm(PWM_PERIOD, keep);
        t.clr_a();
        t.mov_direct_a(self.phase);
        t.sjmp(wrapped);
        t.label(keep);
        t.mov_direct_a(self.phase);
    }

    /// 每个 tick 调一次：按 `phase < duty` 驱动引脚。
    ///
    /// 破坏 `A` 和 `C`。
    pub fn drive(&self, t: &mut Task) {
        t.comment("C = 1 表示 phase < duty —— 借位就是比较结果");
        t.mov_a_direct(self.phase);
        t.clr_c();
        t.subb_a_direct(self.duty);
        t.mov_pin_c(BREATH_PIN);
    }

    /// 一个 PWM 周期结束时调一次：把占空比沿斜坡推一格。
    ///
    /// 破坏 `A` 和 `C`。
    pub fn ramp(&self, t: &mut Task) {
        let done = t.new_label("ramp_done");
        let falling = t.new_label("ramp_falling");
        let at_bottom = t.new_label("ramp_bottom");

        t.comment("每 RAMP_HOLD 个 PWM 周期才动一格占空比");
        t.djnz_direct(self.hold, done);
        t.mov_direct_imm(self.hold, RAMP_HOLD);

        t.mov_a_direct(self.dir);
        t.jnz(falling);

        // ---- 变亮：duty++ ----
        t.inc_direct(self.duty);
        t.mov_a_direct(self.duty);
        t.cjne_a_imm(PWM_PERIOD, done); // 还没到全亮
        t.comment("到全亮了，转向变暗");
        t.mov_direct_imm(self.dir, 1);
        t.sjmp(done);

        // ---- 变暗：duty-- ----
        t.label(falling);
        t.mov_a_direct(self.duty);
        t.jz(at_bottom); // 已经全暗
        t.dec_direct(self.duty);
        t.sjmp(done);

        t.label(at_bottom);
        t.comment("到全暗了，转向变亮");
        t.mov_direct_imm(self.dir, 0);

        t.label(done);
    }
}
