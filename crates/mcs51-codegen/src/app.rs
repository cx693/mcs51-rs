//! 工程构建器：把 Rust 描述编译成固件。

use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};

use mcs51_asm::{AssembleOutput, Options, assemble};
use mcs51_ihex::Image;
use mcs51_ld::{LinkOptions, link};
use mcs51_sim::Machine;

use crate::Board;
use crate::emit::{Helper, Routine, Task};

/// 生成出来的固件（还没落盘）。
#[derive(Debug, Clone)]
pub struct Firmware {
    /// 生成的目标机汇编源码 —— 出问题时直接读这个。
    pub assembly: String,
    /// Intel HEX，用来下载 / 烧录。
    pub hex: String,
    /// 原始二进制。
    pub binary: Vec<u8>,
    /// 链接 MAP。
    pub map: String,
    /// 汇编清单。
    pub listing: String,
    /// 代码镜像，可以直接喂给模拟器。
    pub image: Image,
    /// 板级配置。
    pub board: Board,
    /// 构建期的提醒（不是错误）。
    pub warnings: Vec<String>,
}

impl Firmware {
    /// 把固件写进目录，返回写出的文件列表。
    ///
    /// 产物：`<name>.ihx` / `.bin` / `.map` / `.lst` / `.asm`。
    /// 保留 `.asm` 是为了可追溯 —— 你随时能看到 Rust 究竟生成了什么。
    pub fn write_to(&self, dir: impl AsRef<Path>) -> io::Result<Vec<PathBuf>> {
        let dir = dir.as_ref();
        std::fs::create_dir_all(dir)?;
        let stem = &self.board.name;
        let mut written = Vec::new();
        let files: [(&str, &[u8]); 5] = [
            ("ihx", self.hex.as_bytes()),
            ("bin", &self.binary),
            ("map", self.map.as_bytes()),
            ("lst", self.listing.as_bytes()),
            ("asm", self.assembly.as_bytes()),
        ];
        for (ext, contents) in files {
            let path = dir.join(format!("{stem}.{ext}"));
            std::fs::write(&path, contents)?;
            written.push(path);
        }
        Ok(written)
    }

    /// 一台装好这份固件的模拟器。
    pub fn machine(&self) -> Machine {
        Machine::from_image(&self.image)
    }

    /// 代码字节数。
    pub fn code_size(&self) -> usize {
        self.image.size_report().bytes
    }

    /// 尺寸报告。
    pub fn size_report(&self) -> String {
        let mut out = String::new();
        out.push_str(&format!("mcs51-rs (Rust eDSL): {}\n", self.board.name));
        out.push_str(&format!("  芯片      {}\n", self.board.device));
        out.push_str(&format!(
            "  晶振      {} Hz（1 个机器周期 = {} 个时钟）\n",
            self.board.crystal_hz,
            self.board.clocks_per_cycle()
        ));
        out.push_str(&format!("  串口      {} baud\n", self.board.uart_baud));
        out.push_str(&format!("  代码      {} 字节\n", self.code_size()));
        for warning in &self.warnings {
            out.push_str(&format!("  警告      {warning}\n"));
        }
        out
    }
}

/// 一个 8051 裸机工程。
///
/// 生成的固件是**真正的裸机**：复位向量 + 中断向量表 + 一个主程序，
/// 没有调度器、没有系统节拍、没有运行时。要什么自己用指令搭。
pub struct App {
    board: Board,
    main_body: Option<String>,
    routines: Vec<(String, String)>,
    /// 下一个可用的内部 RAM 字节，用于 [`App::statics`]。
    next_static: u32,
    /// 静态变量区的上界（不含）。
    static_limit: u32,
    /// 已经分配出去的静态字节数。
    statics_used: u8,
    strings: BTreeMap<u32, Vec<u8>>,
    helpers: Vec<Helper>,
    errors: Vec<String>,
    /// 构建期提醒（不是错误）。
    warnings: Vec<String>,
}

impl App {
    /// 建一个裸机工程。
    pub fn new(board: Board) -> Self {
        Self {
            // 0x20 起、0x30 止：0x30 是栈底，8051 的栈向上增长，所以碰不到。
            next_static: 0x20,
            static_limit: 0x30,
            statics_used: 0,
            board,
            main_body: None,
            routines: Vec::new(),
            strings: BTreeMap::new(),
            helpers: Vec::new(),
            errors: Vec::new(),
            warnings: Vec::new(),
        }
    }

    /// 板级配置。
    pub fn board(&self) -> &Board {
        &self.board
    }

    /// 在内部 RAM 里预留一块**目标机静态变量**区，返回起始地址。
    ///
    /// 8051 上变量必须落在固定的直接地址（没有 SP 相对寻址），所以这里返回一个
    /// 地址，你的代码用它做 `MOV direct` / `INC direct`。
    ///
    /// 从 `0x20` 往上分配，到 `0x30` 为止 —— `0x30` 是栈底，而 8051 的栈
    /// **向上增长**，所以两者不会撞上。
    ///
    /// **上界为什么不是 `0x100`**：8051 的直接寻址空间里 `0x80..0xFF` 全是 SFR，
    /// 不是 RAM。`0xD0` 是 PSW，`0xD1..0xD3` 未实现。把变量放在那里，
    /// `MOV direct` 打到的其实是 PSW 和一堆不存在的寄存器 —— 模拟器可能碰巧
    /// 跑得通，真机上必然废。这段区间是 `0x20..0x2F`，正好 16 字节。
    ///
    /// ```
    /// # use mcs51_chips::stc89c5xrc::*;
    /// # use mcs51_codegen::prelude::*;
    /// let mut app = App::new(Board::default());
    /// let pwm = app.statics(4);          // 拿 4 个字节
    /// app.main(move |t| {
    ///     t.mov_direct_imm(pwm, 0);      // pwm+0 = 相位
    ///     t.mov_direct_imm(pwm + 1, 0);  // pwm+1 = 占空比
    ///     t.forever(|t| {
    ///         t.inc_direct(pwm);
    ///     });
    /// });
    /// let fw = app.finish().unwrap();
    /// assert!(fw.assembly.contains("MOV   0x20, #0"));
    /// ```
    pub fn statics(&mut self, bytes: u8) -> u8 {
        let base = self.next_static;
        self.next_static += bytes as u32;
        self.statics_used = self.statics_used.saturating_add(bytes);
        if self.next_static > self.static_limit {
            self.errors.push(format!(
                "静态变量要 {bytes} 字节，但 0x20..0x30 这 16 个字节已经用了 {}；\
                 0x30 以上是栈，0x80 以上是 SFR 不是 RAM。\
                 要更多空间请把数据放到 XDATA（用 DPTR 访问），\
                 或者用寄存器/栈上的临时值。",
                self.statics_used
            ));
        }
        base as u8
    }

    /// 裸机工程的主程序。
    ///
    /// 生成的代码会先设好栈指针再进入 `body`；`body` 返回后程序停在原地。
    /// 所以裸机工程通常以 [`Task::forever`] 结尾。
    pub fn main(&mut self, body: impl FnOnce(&mut Task)) -> &mut Self {
        let mut task = Task::new("__main");
        body(&mut task);
        self.main_body = Some(self.absorb(task));
        self
    }

    /// 定义一个目标机子程序。
    ///
    /// 子程序在目标机上**只生成一份**代码，调用点发 `LCALL`，返回用 `RET`
    /// （自动补在末尾，所以不必自己写）。这和 Rust 普通函数不同 ——
    /// 后者是构建期内联，调用几次就有几份机器码。
    ///
    /// 子程序可以调用其它子程序；参数约定由调用双方自己商定
    /// （典型是 A / R7 / R6 依次放参数，被调用者用 PUSH/POP 保存要用的寄存器）。
    pub fn routine(&mut self, name: &str, body: impl FnOnce(&mut Task)) -> Routine {
        let symbol = format!("__rt_{}", symbol_for(name));
        if self
            .routines
            .iter()
            .any(|(existing, _)| *existing == symbol)
        {
            self.errors.push(format!(
                "子程序 `{name}` 被定义了两次（标签 {symbol} 会冲突）"
            ));
            return Routine::new(name, symbol);
        }
        let mut task = Task::new(name);
        body(&mut task);
        let mut body = self.absorb(task);
        body.push_str("        RET                          ; 子程序返回\n");
        self.routines.push((symbol.clone(), body));
        Routine::new(name, symbol)
    }

    /// 把一段代码里的字符串常量合并到工程级。
    ///
    /// 字符串用内容哈希命名，所以同一个字符串出现多次只会有一份，
    /// 也不需要事后重写下标。
    fn absorb(&mut self, mut task: Task) -> String {
        for bytes in task.strings.drain(..) {
            let key = crate::emit::fnv1a(&bytes);
            self.strings.entry(key).or_insert(bytes);
        }
        for helper in task.helpers.drain(..) {
            if !self.helpers.contains(&helper) {
                self.helpers.push(helper);
            }
        }
        for error in task.errors.drain(..) {
            self.errors.push(error);
        }
        for warning in task.warnings.drain(..) {
            self.warnings.push(warning);
        }
        task.body
    }

    /// 生成目标机汇编源码。
    pub fn generate(&mut self) -> Result<String, String> {
        self.source()
    }

    fn source(&mut self) -> Result<String, String> {
        if !self.errors.is_empty() {
            return Err(self.errors.join("\n"));
        }
        if self.main_body.is_none() {
            return Err("工程必须先调用 App::main".into());
        }

        let mut out = String::new();
        out.push_str("; =====================================================================\n");
        out.push_str(";  由 mcs51-codegen 从 Rust 源码生成 —— 不要手改这个文件。\n");
        out.push_str(";  改 src/main.rs，然后重新构建。\n");
        out.push_str("; =====================================================================\n");
        out.push_str(&format!(";  工程      {}\n", self.board.name));
        out.push_str(&format!(";  芯片      {}\n", self.board.device));
        out.push_str(&format!(
            ";  晶振      {} Hz（1 个机器周期 = {} 个时钟）\n",
            self.board.crystal_hz,
            self.board.clocks_per_cycle()
        ));
        out.push_str(&format!(";  串口      {} baud\n", self.board.uart_baud));
        out.push_str(";  模式      裸机（复位向量 + 向量表 + 主程序）\n");
        out.push_str("; =====================================================================\n\n");

        Self::emit_bare_metal(&mut out, self.main_body.as_deref().unwrap_or(""));
        Self::emit_routines(&mut out, &self.routines);
        Self::emit_data(&mut out, &self.strings);
        Self::emit_helpers(&mut out, &self.helpers);
        out.push_str("\n        .end\n");
        Ok(out)
    }

    fn emit_bare_metal(out: &mut String, main: &str) {
        out.push_str("        .area CODE (ABS)\n");
        out.push_str("        .org  0x0000\n");
        out.push_str("        LJMP  __main                 ; 复位向量\n\n");
        out.push_str("; 中断向量表：全部填 RETI。本工程不用中断，但不能留空 ——\n");
        out.push_str("; 万一有干扰触发中断，落到未初始化的 0xFF 上就会跑飞。\n");
        for (offset, name) in [
            (0x0003u16, "INT0"),
            (0x000B, "Timer0"),
            (0x0013, "INT1"),
            (0x001B, "Timer1"),
            (0x0023, "串口"),
            (0x002B, "Timer2"),
        ] {
            out.push_str(&format!("        .org  0x{offset:04X}\n"));
            out.push_str(&format!("        RETI                         ; {name}\n"));
        }
        out.push('\n');
        out.push_str("        .org  0x0030\n");
        out.push_str("; ---------------------------------------------------------------------\n");
        out.push_str("; 主程序\n");
        out.push_str("; ---------------------------------------------------------------------\n");
        out.push_str("__main:\n");
        out.push_str("        MOV   SP, #0x30              ; 复位后 SP=0x07，必须重设，\n");
        out.push_str("                                     ; 否则第一次压栈会覆盖寄存器组 1\n");
        out.push_str(main);
        out.push_str("__main_exit:\n");
        out.push_str("        SJMP  __main_exit            ; 主程序返回：停在这里\n");
    }

    fn emit_routines(out: &mut String, routines: &[(String, String)]) {
        if routines.is_empty() {
            return;
        }
        out.push_str("; ---------------------------------------------------------------------\n");
        out.push_str("; 子程序：目标机上只生成一份，用 LCALL 调用\n");
        out.push_str("; ---------------------------------------------------------------------\n");
        for (symbol, body) in routines {
            out.push_str(&format!("{symbol}:\n"));
            out.push_str(body);
            out.push('\n');
        }
    }

    fn emit_data(out: &mut String, strings: &BTreeMap<u32, Vec<u8>>) {
        if strings.is_empty() {
            return;
        }
        out.push_str("; ---------------------------------------------------------------------\n");
        out.push_str("; 字符串常量（CODE 段，用 MOVC A, @A+DPTR 读取）\n");
        out.push_str("; ---------------------------------------------------------------------\n");
        for (key, bytes) in strings {
            let text: String = bytes
                .iter()
                .filter(|b| **b != 0)
                .map(|b| match *b {
                    b'"' => "\\\"".to_string(),
                    b'\\' => "\\\\".to_string(),
                    b'\r' => "\\r".to_string(),
                    b'\n' => "\\n".to_string(),
                    b'\t' => "\\t".to_string(),
                    0x20..=0x7E => (*b as char).to_string(),
                    other => format!("\\x{other:02X}"),
                })
                .collect();
            out.push_str(&format!("__str_{key:08X}: .db  \"{text}\", 0\n"));
        }
        out.push('\n');
    }

    fn emit_helpers(out: &mut String, helpers: &[Helper]) {
        if helpers.is_empty() {
            return;
        }
        out.push_str("; ---------------------------------------------------------------------\n");
        out.push_str("; 由 mcs51-codegen 生成的小工具例程\n");
        out.push_str("; ---------------------------------------------------------------------\n");
        for helper in helpers {
            match helper {
                Helper::DelayMs => out.push_str(DELAY_MS),
            }
        }
    }

    /// 编译、链接，产出固件。
    pub fn finish(&mut self) -> Result<Firmware, String> {
        let source = self.source()?;
        self.assemble_and_link(source)
    }

    fn assemble_and_link(&self, source: String) -> Result<Firmware, String> {
        let output = assemble(&source, &Options::new(format!("{}.asm", self.board.name)));
        if !output.is_ok() {
            return Err(format!(
                "生成的汇编没通过汇编器（这是 mcs51-codegen 的 bug，请报告）：\n{}",
                output.render_diagnostics()
            ));
        }
        let objects = [output.object.clone()];
        let linked = link(&objects, &LinkOptions::default());
        if !linked.is_ok() {
            return Err(format!("链接失败：\n{}", linked.render_diagnostics()));
        }
        Ok(self.collect(source, listing_text(&output), linked, &objects))
    }

    fn collect(
        &self,
        source: String,
        listing: String,
        linked: mcs51_ld::LinkOutput,
        objects: &[mcs51_asm::ObjectFile],
    ) -> Firmware {
        Firmware {
            hex: linked.code.to_intel_hex(),
            binary: linked.code.to_binary(0xFF).unwrap_or_default(),
            map: linked.map_file(objects),
            listing,
            image: linked.code.clone(),
            assembly: source,
            board: self.board.clone(),
            warnings: Vec::new(),
        }
    }
}

/// 把子程序名变成合法的汇编标签（`led.all_off` → `led_all_off`）。
fn symbol_for(name: &str) -> String {
    name.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect()
}

fn listing_text(output: &AssembleOutput) -> String {
    let mut out = String::new();
    for line in &output.listing {
        let bytes = line
            .bytes
            .iter()
            .map(|b| format!("{b:02X}"))
            .collect::<Vec<_>>()
            .join(" ");
        match line.addr {
            Some(addr) if !line.bytes.is_empty() => {
                out.push_str(&format!(
                    "{addr:04X}  {bytes:<12} {}\n",
                    line.text.trim_end()
                ));
            }
            _ => out.push_str(&format!("      {:<12} {}\n", "", line.text.trim_end())),
        }
    }
    out
}

const DELAY_MS: &str = "\
; __delay_ms：A = 毫秒数，忙等。按 11.0592MHz 标定（1ms ≈ 921 个机器周期）。\n\
; 会保存/恢复 R1、R2，调用者的寄存器不受影响。\n\
__delay_ms:\n\
        PUSH  0x01\n\
        PUSH  0x02\n\
        MOV   R1, A\n\
__delay_ms_outer:\n\
        MOV   R2, #230               ; 230 × 4 周期 ≈ 920 周期 ≈ 1ms\n\
__delay_ms_inner:\n\
        NOP\n\
        NOP\n\
        DJNZ  R2, __delay_ms_inner\n\
        DJNZ  R1, __delay_ms_outer\n\
        POP   0x02\n\
        POP   0x01\n\
        RET\n\n";
