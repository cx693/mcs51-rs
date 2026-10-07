//! 目标机子程序：只生成一份代码，调用点是 `LCALL`。
//!
//! 这是和「Rust 普通函数」的关键区别 —— 后者是构建期内联，调用 N 次就有
//! N 份机器码。真实固件里的驱动和公共算法必须走子程序，否则 Flash 会被
//! 重复代码吃光。

use mcs51_chips::stc89c5xrc::*;
use mcs51_codegen::prelude::*;

/// 建一个裸机工程，主程序里用 `body` 拼代码。
fn app_with(body: impl FnOnce(&mut Task)) -> Firmware {
    let mut app = App::new(Board::default());
    app.main(body);
    app.finish().expect("工程应该能生成")
}

#[test]
fn a_routine_is_emitted_once_and_called_with_lcall() {
    let mut app = App::new(Board::default());

    let all_off = app.routine("all_off", |t| {
        for pin in [P35, P36, P37] {
            t.setb(pin);
        }
    });

    app.main(move |t| {
        // 三个调用点 —— 目标机上仍然只有一份 all_off。
        t.call_routine(&all_off);
        t.call_routine(&all_off);
        t.call_routine(&all_off);
        t.forever(|t| {
            t.nop();
        });
    });

    let firmware = app.finish().unwrap();
    assert_eq!(
        firmware.assembly.matches("__rt_all_off:").count(),
        1,
        "子程序必须只生成一份：\n{}",
        firmware.assembly
    );
    assert_eq!(
        firmware.assembly.matches("LCALL __rt_all_off").count(),
        3,
        "三个调用点都要发 LCALL"
    );
    // 末尾自动补 RET。
    assert!(firmware.assembly.contains("; 子程序返回"));
}

#[test]
fn calling_a_routine_does_not_duplicate_its_code() {
    /// 一个 Rust 普通函数：构建期被内联。
    fn inline_all_off(t: &mut Task) {
        for pin in [P35, P36, P37] {
            t.setb(pin);
        }
    }

    // 走子程序：调用三次，代码只有一份。
    let mut app = App::new(Board::default());
    let all_off = app.routine("all_off", inline_all_off);
    app.main(move |t| {
        t.call_routine(&all_off);
        t.call_routine(&all_off);
        t.call_routine(&all_off);
        t.forever(|t| {
            t.nop();
        });
    });
    let with_routine = app.finish().unwrap();

    // 直接内联：调用三次，机器码三份。
    let with_inline = app_with(|t| {
        inline_all_off(t);
        inline_all_off(t);
        inline_all_off(t);
        t.forever(|t| {
            t.nop();
        });
    });

    assert_eq!(
        with_routine.assembly.matches("SETB  P35").count(),
        1,
        "子程序版本里 SETB P35 只应该出现一次"
    );
    assert_eq!(
        with_inline.assembly.matches("SETB  P35").count(),
        3,
        "内联版本里出现三次 —— 这正是子程序要解决的问题"
    );
    assert!(
        with_routine.code_size() < with_inline.code_size(),
        "子程序版本应该更小：{} vs {}",
        with_routine.code_size(),
        with_inline.code_size()
    );
}

#[test]
fn routines_can_call_each_other_and_the_delay_helper() {
    let mut app = App::new(Board::default());

    let step = app.routine("step", |t| {
        t.clr(P35);
        t.delay_ms(10);
        t.setb(P35);
    });
    let cycle = app.routine("cycle", |t| {
        t.call_routine(&step);
        t.call_routine(&step);
        t.call_routine(&step);
    });

    app.main(move |t| {
        t.forever(|t| {
            t.call_routine(&cycle);
        });
    });

    let firmware = app.finish().unwrap();
    assert!(firmware.assembly.contains("__rt_step:"));
    assert!(firmware.assembly.contains("__rt_cycle:"));
    assert_eq!(firmware.assembly.matches("LCALL __rt_step").count(), 3);
    // delay_ms 是 codegen 生成的小工具例程，子程序里也能调。
    assert!(firmware.assembly.contains("LCALL __delay_ms"));
    assert!(firmware.assembly.contains("__delay_ms:"));
}

#[test]
fn a_duplicate_routine_name_is_a_build_error() {
    let mut app = App::new(Board::default());
    app.routine("same", |t| {
        t.nop();
    });
    app.routine("same", |t| {
        t.nop();
    });
    app.main(|t| {
        t.forever(|t| {
            t.nop();
        });
    });
    let error = app.finish().unwrap_err();
    assert!(error.contains("定义了两次"), "{error}");
}

#[test]
fn a_routine_can_be_a_tail_call_target() {
    let mut app = App::new(Board::default());
    let handler = app.routine("handler", |t| {
        t.nop();
    });
    app.main(move |t| {
        t.forever(|t| {
            t.tail_call(&handler); // LJMP，不再是 LCALL
            t.nop();
        });
    });
    let firmware = app.finish().unwrap();
    assert!(firmware.assembly.contains("LJMP  __rt_handler"));
    assert!(!firmware.assembly.contains("LCALL __rt_handler"));
}

#[test]
fn a_routine_body_is_generated_after_the_main_program() {
    // 子程序必须独立成段，不能混进主程序的 `forever` 里，
    // 否则 `RET` 会落在死循环后面、永远回不来。
    let mut app = App::new(Board::default());
    let sub = app.routine("sub", |t| {
        t.nop();
    });
    app.main(move |t| {
        t.call_routine(&sub);
        t.forever(|t| {
            t.nop();
        });
    });
    let firmware = app.finish().unwrap();
    let asm = &firmware.assembly;
    let main_exit = asm.find("__main_exit").expect("主程序出口");
    let routine = asm.find("__rt_sub:").expect("子程序");
    assert!(routine > main_exit, "子程序应该排在主程序之后：\n{asm}");
}
