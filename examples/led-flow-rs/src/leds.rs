//! 流水灯模块：P3.5 和 P3.6 轮流点亮。
//!
//! P3.7 不在这个模块里 —— 它被呼吸灯（[`crate::breath`]）占了。
//!
//! 换灯的时刻由 Timer 0 数出来（每 `CHASE_PERIODS` 个 PWM 周期），
//! 所以流水灯和呼吸灯共用同一个时基，不需要第二个定时器，也不需要 `delay` 死等。

use mcs51_codegen::prelude::*;

use crate::board::{CHASE_LEDS, CHASE_PERIODS};

/// 流水灯的目标机状态（内部 RAM 地址 + 常量）。
#[derive(Debug, Clone, Copy)]
pub struct Chaser {
    /// 还有几个 PWM 周期换下一只灯。
    count: u8,
    /// 当前轮到第几只（0 或 1）。
    step: u8,
}

impl Chaser {
    /// `base` 是 [`App::statics`] 分配的 2 个字节。
    pub fn new(base: u8) -> Self {
        Self {
            count: base,
            step: base + 1,
        }
    }

    /// 初始化：先全灭，从第 0 只开始。
    pub fn init(&self, t: &mut Task) {
        t.comment("上电先把两只流水灯熄灭，避免下载完的一瞬间乱亮");
        for pin in CHASE_LEDS {
            t.setb(pin);
        }
        t.mov_direct_imm(self.count, CHASE_PERIODS);
        t.mov_direct_imm(self.step, 0);
    }

    /// 一个 PWM 周期结束时调一次：到点就换灯。
    ///
    /// 破坏 `A`。
    pub fn tick(&self, t: &mut Task) {
        let done = t.new_label("chase_done");
        let second = t.new_label("chase_second");

        t.djnz_direct(self.count, done);
        t.mov_direct_imm(self.count, CHASE_PERIODS);

        t.mov_a_direct(self.step);
        t.jnz(second);

        // ---- 轮到第 0 只 ----
        //
        // 先灭掉上一只，再点亮这一只。反过来写（先亮后灭）会有一瞬间两只都亮，
        // 虽然只有一两个机器周期、肉眼看不见，但「任何时刻只有一只亮」是个
        // 值得保持的不变式，测试也盯着它。
        t.setb(CHASE_LEDS[1]);
        t.clr(CHASE_LEDS[0]);
        t.mov_direct_imm(self.step, 1);
        t.sjmp(done);

        // ---- 轮到第 1 只 ----
        t.label(second);
        t.setb(CHASE_LEDS[0]);
        t.clr(CHASE_LEDS[1]);
        t.mov_direct_imm(self.step, 0);

        t.label(done);
    }
}
