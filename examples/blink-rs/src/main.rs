//! 入口只有三行 —— 全部逻辑在 [`blink_rs`] 库里，方便测试直接调用。

fn main() {
    if let Err(error) = blink_rs::emit(blink_rs::build(&blink_rs::project_name("blink-rs"))) {
        eprintln!("{error}");
        std::process::exit(1);
    }
}
