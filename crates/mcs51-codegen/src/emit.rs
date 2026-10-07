//! 指令发射器：用类型安全的 Rust API 生成 8051 汇编。
//!
//! 这里生成的**是真实的 8051 汇编源码**，交给本仓库的汇编器处理。
//! 这么做有个额外好处：出问题时你能直接读到生成结果，而不是面对一堆二进制。
//!
//! 命名约定：
//!
//! * `mov_a_imm(v)`   → `MOV A, #v`
//! * `mov_reg_imm(r, v)` → `MOV Rn, #v`
//! * `add_a_reg(r)`   → `ADD A, Rn`
//! * `pin_low(p)`     → `CLR P35`
//! * `jz(label)`      → `JZ label`

use core::fmt::Write as _;

/// 一个位寻址单元：端口引脚（`P35`）或 SFR 里的位（`TF0`、`TR0`、`EA`…）。
///
/// 类型来自芯片标准库 [`mcs51_chips`]：8051 的位寻址空间是统一的，
/// `SETB`/`CLR`/`JB`/`MOV C,bit` 对 RAM 位和 SFR 位一视同仁，所以只用一种类型。
pub use mcs51_chips::Bit;

// 这里**不**导出任何 SFR 位的名字（`TR0`、`TF0`、`EA`…）。
//
// 那些是**芯片相关**的：8051 基础款有 reg52 的那一套，STC 还会多出 P4、ISP_*、
// 看门狗。eDSL 只管"怎么生成代码"，不该认识某颗具体的芯片 ——
// 否则 `mcs51_codegen::TR0` 会固定指向 reg52，而应用用的是 stc89c5xrc，
// 又变成两个地址来源。
//
// 应用这样导入就够了：
//
// ```
// use mcs51_chips::stc89c5xrc::*;   // 硬件：P35, TR0, TMOD, ISP_DATA…
// use mcs51_codegen::prelude::*;        // eDSL：App, Task, Bit, Reg, Label
// ```

/// 工作寄存器 `R0`..`R7`（bank 0）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Reg {
    /// `R0`
    R0,
    /// `R1`
    R1,
    /// `R2`
    R2,
    /// `R3`
    R3,
    /// `R4`
    R4,
    /// `R5`
    R5,
    /// `R6`
    R6,
    /// `R7`
    R7,
}

impl Reg {
    /// 汇编里的名字。
    pub const fn as_str(self) -> &'static str {
        match self {
            Reg::R0 => "R0",
            Reg::R1 => "R1",
            Reg::R2 => "R2",
            Reg::R3 => "R3",
            Reg::R4 => "R4",
            Reg::R5 => "R5",
            Reg::R6 => "R6",
            Reg::R7 => "R7",
        }
    }

    /// 在 bank 0 里的直接地址，用于 `PUSH`/`POP`（它们只接受直接地址）。
    pub const fn direct(self) -> u8 {
        match self {
            Reg::R0 => 0x00,
            Reg::R1 => 0x01,
            Reg::R2 => 0x02,
            Reg::R3 => 0x03,
            Reg::R4 => 0x04,
            Reg::R5 => 0x05,
            Reg::R6 => 0x06,
            Reg::R7 => 0x07,
        }
    }
}

/// 一个跳转目标。
///
/// 用 [`Task::new_label`] 创建，用 [`Task::label`] 落地，用各种跳转指令引用。
/// 用类型而不是字符串，是为了让「跳到一个拼错的标签」变成编译错误。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Label(usize);

impl Label {
    /// 标签的内部编号。
    pub const fn id(self) -> usize {
        self.0
    }
}

/// 目标机上的一个**子程序**：只生成一份代码，用 `LCALL` 调用。
///
/// 这是和「Rust 普通函数」的关键区别：
///
/// * Rust 普通函数 `fn foo(t: &mut Task)` 是在**构建期**被调用的，
///   它的代码会被**内联展开**到每一个调用点 —— 调用 5 次就有 5 份机器码。
/// * [`Routine`] 在目标机上只有**一份**代码，调用点发的是 `LCALL`。
///
/// 真实的固件开发（驱动、协议栈、公共算法）必须用后者，否则 Flash 会被
/// 重复代码吃光。用 [`App::routine`] 定义，用 [`Task::call_routine`] 调用。
///
/// ```
/// # use mcs51_chips::stc89c5xrc::*;
/// # use mcs51_codegen::prelude::*;
/// let mut app = App::new(Board::default());
/// // 定义一次
/// let all_off = app.routine("all_off", |t| {
///     for pin in [P35, P36, P37] { t.setb(pin); }
/// });
/// // 主程序和别的子程序都能调用它
/// app.main(move |t| {
///     t.call_routine(&all_off);
///     t.forever(|t| { t.nop(); });
/// });
/// let fw = app.finish().unwrap();
/// // 目标机上只有一份 all_off
/// assert_eq!(fw.assembly.matches("__rt_all_off:").count(), 1);
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Routine {
    name: String,
    symbol: String,
}

impl Routine {
    pub(crate) fn new(name: &str, symbol: String) -> Self {
        Self {
            name: name.to_string(),
            symbol,
        }
    }

    /// 定义时用的名字。
    pub fn name(&self) -> &str {
        &self.name
    }

    /// 生成的汇编标签，例如 `__rt_all_off`。
    pub fn symbol(&self) -> &str {
        &self.symbol
    }
}

/// 目标机构建过程中需要生成的小工具例程。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
pub(crate) enum Helper {
    /// `__delay_ms`：A = 毫秒数，忙等。
    #[default]
    DelayMs,
}

/// FNV-1a：给字符串常量起名，同样的内容得到同样的标签。
pub(crate) fn fnv1a(bytes: &[u8]) -> u32 {
    let mut hash = 0x811C_9DC5u32;
    for byte in bytes {
        hash ^= *byte as u32;
        hash = hash.wrapping_mul(0x0100_0193);
    }
    hash
}

/// 生成一个引脚的直接地址写法（用于 `MOV direct, ...`）。
fn direct_text(address: impl Into<u8>) -> String {
    let address: u8 = address.into();
    match mcs51_isa::sfr::direct_name(address) {
        Some(name) => name.to_string(),
        None => format!("0x{address:02X}"),
    }
}

/// 一段目标机代码。
///
/// 传给 [`crate::App::task`] 或 [`crate::App::main`] 的闭包会拿到它。
/// 所有方法都是「发一条指令」，按调用顺序生成。
pub struct Task {
    pub(crate) name: String,
    pub(crate) body: String,
    next_label: usize,
    label_names: Vec<String>,
    pub(crate) strings: Vec<Vec<u8>>,
    pub(crate) helpers: Vec<Helper>,
    pub(crate) errors: Vec<String>,
    pub(crate) warnings: Vec<String>,
}

impl Task {
    pub(crate) fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            body: String::new(),
            next_label: 0,
            label_names: Vec::new(),
            strings: Vec::new(),
            helpers: Vec::new(),
            errors: Vec::new(),
            warnings: Vec::new(),
        }
    }

    // -----------------------------------------------------------------------
    // 底层：直接发一行汇编
    // -----------------------------------------------------------------------

    /// 原样发一条汇编指令。用于本 crate 还没包装的指令。
    ///
    /// ```
    /// # use mcs51_chips::stc89c5xrc::*;
    /// # use mcs51_codegen::{App, Board};
    /// let mut app = App::new(Board::default());
    /// app.main(|t| {
    ///     t.raw("MOV  A, #0x42");
    ///     t.raw("SJMP $");
    /// });
    /// ```
    pub fn raw(&mut self, instruction: &str) -> &mut Self {
        let _ = writeln!(self.body, "        {instruction}");
        self
    }

    /// 发一条带注释的汇编（注释会出现在生成的 .asm 里）。
    pub fn raw_commented(&mut self, comment: &str, instruction: &str) -> &mut Self {
        let _ = writeln!(self.body, "        {instruction:<28} ; {comment}");
        self
    }

    /// 只发一条注释。
    pub fn comment(&mut self, comment: &str) -> &mut Self {
        let _ = writeln!(self.body, "; {comment}");
        self
    }

    /// 空行。
    pub fn blank(&mut self) -> &mut Self {
        self.body.push('\n');
        self
    }

    fn need(&mut self, helper: Helper) {
        if !self.helpers.contains(&helper) {
            self.helpers.push(helper);
        }
    }

    pub(crate) fn fail(&mut self, message: impl Into<String>) {
        self.errors.push(message.into());
    }

    /// 构建期提醒 —— 不是错误，但值得让用户知道。
    pub(crate) fn warn(&mut self, message: impl Into<String>) {
        self.warnings.push(message.into());
    }

    // -----------------------------------------------------------------------
    // 标签与控制流
    // -----------------------------------------------------------------------

    /// 申请一个新标签（此时还不落地，只是拿到句柄）。
    ///
    /// `hint` 只影响生成汇编里的可读性，生成的名字形如 `__led_task_0_loop`。
    pub fn new_label(&mut self, hint: &str) -> Label {
        let id = self.next_label;
        self.next_label += 1;
        let safe: String = hint
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
            .collect();
        self.label_names
            .push(format!("__{}_{id}_{safe}", self.name));
        Label(id)
    }

    /// 把标签落在当前位置。
    pub fn label(&mut self, label: Label) -> &mut Self {
        let name = self.label_names[label.0].clone();
        let _ = writeln!(self.body, "{name}:");
        self
    }

    fn label_ref(&self, label: Label) -> String {
        self.label_names[label.0].clone()
    }

    /// 无条件短跳转 `SJMP`。
    pub fn sjmp(&mut self, label: Label) -> &mut Self {
        let target = self.label_ref(label);
        self.raw(&format!("SJMP  {target}"))
    }

    /// `JZ`：A 为 0 时跳转。
    pub fn jz(&mut self, label: Label) -> &mut Self {
        let target = self.label_ref(label);
        self.raw(&format!("JZ    {target}"))
    }

    /// `JNZ`：A 不为 0 时跳转。
    pub fn jnz(&mut self, label: Label) -> &mut Self {
        let target = self.label_ref(label);
        self.raw(&format!("JNZ   {target}"))
    }

    /// `JC`：进位为 1 时跳转。
    pub fn jc(&mut self, label: Label) -> &mut Self {
        let target = self.label_ref(label);
        self.raw(&format!("JC    {target}"))
    }

    /// `JNC`：进位为 0 时跳转。
    pub fn jnc(&mut self, label: Label) -> &mut Self {
        let target = self.label_ref(label);
        self.raw(&format!("JNC   {target}"))
    }

    /// `JB`：引脚为 1 时跳转。
    pub fn jb(&mut self, pin: Bit, label: Label) -> &mut Self {
        let target = self.label_ref(label);
        self.raw(&format!("JB    {:<6}, {target}", pin.name))
    }

    /// `JNB`：引脚为 0 时跳转。
    pub fn jnb(&mut self, pin: Bit, label: Label) -> &mut Self {
        let target = self.label_ref(label);
        self.raw(&format!("JNB   {:<6}, {target}", pin.name))
    }

    /// `DJNZ Rn, label`：寄存器减 1，不为 0 就跳转。
    pub fn djnz_reg(&mut self, reg: Reg, label: Label) -> &mut Self {
        let target = self.label_ref(label);
        self.raw(&format!("DJNZ  {:<6}, {target}", reg.as_str()))
    }

    /// `DJNZ direct, label`。
    pub fn djnz_direct(&mut self, direct: impl Into<u8>, label: Label) -> &mut Self {
        let target = self.label_ref(label);
        self.raw(&format!("DJNZ  {:<6}, {target}", direct_text(direct)))
    }

    /// `CJNE A, #imm, label`。
    pub fn cjne_a_imm(&mut self, value: u8, label: Label) -> &mut Self {
        let target = self.label_ref(label);
        self.raw(&format!("CJNE  A, #{value}, {target}"))
    }

    /// `CJNE Rn, #imm, label`。
    pub fn cjne_reg_imm(&mut self, reg: Reg, value: u8, label: Label) -> &mut Self {
        let target = self.label_ref(label);
        self.raw(&format!("CJNE  {:<6}, #{value}, {target}", reg.as_str()))
    }

    /// 目标机上的死循环：先落地标签，生成 `body`，末尾 `SJMP` 回到标签。
    ///
    /// 这是裸机固件里唯一的"结构化循环" —— Rust 的 `loop`/`while` 是在
    /// **主机上**求值的，不会变成目标机的跳转。想在目标机上循环，就用它。
    ///
    /// 闭包体**要用花括号**：`|t| { t.clr(P35); }`。
    /// `|t| t.clr(P35)` 编译不过 —— 链式调用返回 `&mut Task`，而闭包需要
    /// 返回 `()`。链式写法在块里依然可以用：
    /// `|t| { t.clr(P35).setb(P36); }`。
    pub fn forever(&mut self, body: impl FnOnce(&mut Task)) -> &mut Self {
        let top = self.new_label("forever");
        self.label(top);
        body(self);
        self.sjmp(top)
    }

    // -----------------------------------------------------------------------
    // 数据传送
    // -----------------------------------------------------------------------

    /// `MOV A, #imm`
    pub fn mov_a_imm(&mut self, value: u8) -> &mut Self {
        self.raw(&format!("MOV   A, #{value}"))
    }

    /// `MOV A, direct`
    pub fn mov_a_direct(&mut self, direct: impl Into<u8>) -> &mut Self {
        self.raw(&format!("MOV   A, {}", direct_text(direct)))
    }

    /// `MOV A, Rn`
    pub fn mov_a_reg(&mut self, reg: Reg) -> &mut Self {
        self.raw(&format!("MOV   A, {}", reg.as_str()))
    }

    /// `MOV direct, A`
    pub fn mov_direct_a(&mut self, direct: impl Into<u8>) -> &mut Self {
        self.raw(&format!("MOV   {}, A", direct_text(direct)))
    }

    /// `MOV direct, #imm`
    pub fn mov_direct_imm(&mut self, direct: impl Into<u8>, value: u8) -> &mut Self {
        self.raw(&format!("MOV   {}, #{value}", direct_text(direct)))
    }

    /// `MOV direct, Rn`
    pub fn mov_direct_reg(&mut self, direct: impl Into<u8>, reg: Reg) -> &mut Self {
        self.raw(&format!("MOV   {}, {}", direct_text(direct), reg.as_str()))
    }

    /// `MOV direct, direct`
    pub fn mov_direct_direct(
        &mut self,
        destination: impl Into<u8>,
        source: impl Into<u8>,
    ) -> &mut Self {
        self.raw(&format!(
            "MOV   {}, {}",
            direct_text(destination),
            direct_text(source)
        ))
    }

    /// `MOV Rn, #imm`
    pub fn mov_reg_imm(&mut self, reg: Reg, value: u8) -> &mut Self {
        self.raw(&format!("MOV   {}, #{value}", reg.as_str()))
    }

    /// `MOV Rn, A`
    pub fn mov_reg_a(&mut self, reg: Reg) -> &mut Self {
        self.raw(&format!("MOV   {}, A", reg.as_str()))
    }

    /// `MOV Rn, direct`
    pub fn mov_reg_direct(&mut self, reg: Reg, direct: impl Into<u8>) -> &mut Self {
        self.raw(&format!("MOV   {}, {}", reg.as_str(), direct_text(direct)))
    }

    /// `MOV DPTR, #imm16`
    pub fn mov_dptr(&mut self, value: u16) -> &mut Self {
        self.raw(&format!("MOV   DPTR, #0x{value:04X}"))
    }

    /// `MOV DPTR, #symbol`
    pub fn mov_dptr_symbol(&mut self, symbol: &str) -> &mut Self {
        self.raw(&format!("MOV   DPTR, #{symbol}"))
    }

    /// `MOVX A, @DPTR`
    pub fn movx_a_dptr(&mut self) -> &mut Self {
        self.raw("MOVX  A, @DPTR")
    }

    /// `MOVX @DPTR, A`
    pub fn movx_dptr_a(&mut self) -> &mut Self {
        self.raw("MOVX  @DPTR, A")
    }

    /// `MOVX A, @Ri`
    pub fn movx_a_ri(&mut self, ri: Reg) -> &mut Self {
        self.raw(&format!("MOVX  A, @{}", ri.as_str()))
    }

    /// `MOVX @Ri, A`
    pub fn movx_ri_a(&mut self, ri: Reg) -> &mut Self {
        self.raw(&format!("MOVX  @{}, A", ri.as_str()))
    }

    /// `MOVC A, @A+DPTR`
    pub fn movc_a_dptr(&mut self) -> &mut Self {
        self.raw("MOVC  A, @A+DPTR")
    }

    /// `PUSH Rn`（bank 0）
    pub fn push_reg(&mut self, reg: Reg) -> &mut Self {
        self.raw(&format!("PUSH  {}", direct_text(reg.direct())))
    }

    /// `POP Rn`（bank 0）
    pub fn pop_reg(&mut self, reg: Reg) -> &mut Self {
        self.raw(&format!("POP   {}", direct_text(reg.direct())))
    }

    /// `PUSH direct`
    pub fn push_direct(&mut self, direct: impl Into<u8>) -> &mut Self {
        self.raw(&format!("PUSH  {}", direct_text(direct)))
    }

    /// `POP direct`
    pub fn pop_direct(&mut self, direct: impl Into<u8>) -> &mut Self {
        self.raw(&format!("POP   {}", direct_text(direct)))
    }

    // -----------------------------------------------------------------------
    // 算术与逻辑
    // -----------------------------------------------------------------------

    /// `ADD A, #imm`
    pub fn add_a_imm(&mut self, value: u8) -> &mut Self {
        self.raw(&format!("ADD   A, #{value}"))
    }

    /// `ADD A, Rn`
    pub fn add_a_reg(&mut self, reg: Reg) -> &mut Self {
        self.raw(&format!("ADD   A, {}", reg.as_str()))
    }

    /// `ADDC A, #imm`
    pub fn addc_a_imm(&mut self, value: u8) -> &mut Self {
        self.raw(&format!("ADDC  A, #{value}"))
    }

    /// `SUBB A, #imm`
    pub fn subb_a_imm(&mut self, value: u8) -> &mut Self {
        self.raw(&format!("SUBB  A, #{value}"))
    }

    /// `SUBB A, direct` —— 配合 [`Task::clr_c`] 就是一次不带借位的比较：
    /// 借位 C = 1 表示 `A < direct`。软件 PWM 判断 `phase < duty` 就靠它。
    pub fn subb_a_direct(&mut self, direct: impl Into<u8>) -> &mut Self {
        self.raw(&format!("SUBB  A, {}", direct_text(direct)))
    }

    /// `ADD A, direct`
    pub fn add_a_direct(&mut self, direct: impl Into<u8>) -> &mut Self {
        self.raw(&format!("ADD   A, {}", direct_text(direct)))
    }

    /// `MOV direct, direct` 的别名，读起来更顺。
    pub fn copy(&mut self, destination: impl Into<u8>, source: impl Into<u8>) -> &mut Self {
        self.mov_direct_direct(destination, source)
    }

    /// `ANL A, #imm`
    pub fn anl_a_imm(&mut self, value: u8) -> &mut Self {
        self.raw(&format!("ANL   A, #{value}"))
    }

    /// `ORL A, #imm`
    pub fn orl_a_imm(&mut self, value: u8) -> &mut Self {
        self.raw(&format!("ORL   A, #{value}"))
    }

    /// `ANL direct, A` —— 直接地址的读-改-写，8051 没有 `ANL direct, #imm`。
    ///
    /// 典型用法是改一个寄存器的一半位而不影响另一半，例如 `TMOD` 的低 4 位
    /// 属于 Timer 0、高 4 位属于 Timer 1。
    pub fn anl_direct_a(&mut self, direct: impl Into<u8>) -> &mut Self {
        self.raw(&format!("ANL   {}, A", direct_text(direct)))
    }

    /// `ORL direct, A`
    pub fn orl_direct_a(&mut self, direct: impl Into<u8>) -> &mut Self {
        self.raw(&format!("ORL   {}, A", direct_text(direct)))
    }

    /// `XRL A, #imm`
    pub fn xrl_a_imm(&mut self, value: u8) -> &mut Self {
        self.raw(&format!("XRL   A, #{value}"))
    }

    /// `INC A`
    pub fn inc_a(&mut self) -> &mut Self {
        self.raw("INC   A")
    }

    /// `DEC A`
    pub fn dec_a(&mut self) -> &mut Self {
        self.raw("DEC   A")
    }

    /// `INC Rn`
    pub fn inc_reg(&mut self, reg: Reg) -> &mut Self {
        self.raw(&format!("INC   {}", reg.as_str()))
    }

    /// `DEC Rn`
    pub fn dec_reg(&mut self, reg: Reg) -> &mut Self {
        self.raw(&format!("DEC   {}", reg.as_str()))
    }

    /// `INC direct`
    pub fn inc_direct(&mut self, direct: impl Into<u8>) -> &mut Self {
        self.raw(&format!("INC   {}", direct_text(direct)))
    }

    /// `DEC direct`
    pub fn dec_direct(&mut self, direct: impl Into<u8>) -> &mut Self {
        self.raw(&format!("DEC   {}", direct_text(direct)))
    }

    /// `INC DPTR`
    pub fn inc_dptr(&mut self) -> &mut Self {
        self.raw("INC   DPTR")
    }

    /// `MUL AB` —— `A × B → A(低) : B(高)`
    pub fn mul_ab(&mut self) -> &mut Self {
        self.raw("MUL   AB")
    }

    /// `DIV AB` —— `A / B → 商在 A、余数在 B`
    pub fn div_ab(&mut self) -> &mut Self {
        self.raw("DIV   AB")
    }

    /// `MOV B, #imm`。
    pub fn mov_b_imm(&mut self, value: u8) -> &mut Self {
        self.raw(&format!("MOV   B, #{value}"))
    }

    /// `MOV A, B`。
    pub fn mov_a_b(&mut self) -> &mut Self {
        self.raw("MOV   A, B")
    }

    /// `MOV B, A`。
    pub fn mov_b_a(&mut self) -> &mut Self {
        self.raw("MOV   B, A")
    }

    /// `MOV Rn, B`。
    pub fn mov_reg_b(&mut self, reg: Reg) -> &mut Self {
        self.raw(&format!("MOV   {}, B", reg.as_str()))
    }

    /// `DA A`
    pub fn da_a(&mut self) -> &mut Self {
        self.raw("DA    A")
    }

    /// `CLR A`
    pub fn clr_a(&mut self) -> &mut Self {
        self.raw("CLR   A")
    }

    /// `CPL A`
    pub fn cpl_a(&mut self) -> &mut Self {
        self.raw("CPL   A")
    }

    /// `RL A`
    pub fn rl_a(&mut self) -> &mut Self {
        self.raw("RL    A")
    }

    /// `RR A`
    pub fn rr_a(&mut self) -> &mut Self {
        self.raw("RR    A")
    }

    /// `RLC A`
    pub fn rlc_a(&mut self) -> &mut Self {
        self.raw("RLC   A")
    }

    /// `RRC A`
    pub fn rrc_a(&mut self) -> &mut Self {
        self.raw("RRC   A")
    }

    /// `SWAP A`
    pub fn swap_a(&mut self) -> &mut Self {
        self.raw("SWAP  A")
    }

    // -----------------------------------------------------------------------
    // 位操作
    // -----------------------------------------------------------------------

    /// `SETB pin`
    pub fn setb(&mut self, pin: Bit) -> &mut Self {
        self.raw(&format!("SETB  {}", pin.name))
    }

    /// `CLR pin`
    pub fn clr(&mut self, pin: Bit) -> &mut Self {
        self.raw(&format!("CLR   {}", pin.name))
    }

    /// `CPL pin`
    pub fn cpl(&mut self, pin: Bit) -> &mut Self {
        self.raw(&format!("CPL   {}", pin.name))
    }

    /// `SETB C`
    pub fn setb_c(&mut self) -> &mut Self {
        self.raw("SETB  C")
    }

    /// `CLR C`
    pub fn clr_c(&mut self) -> &mut Self {
        self.raw("CLR   C")
    }

    /// `CPL C`
    pub fn cpl_c(&mut self) -> &mut Self {
        self.raw("CPL   C")
    }

    /// `ANL C, pin`
    pub fn anl_c_pin(&mut self, pin: Bit) -> &mut Self {
        self.raw(&format!("ANL   C, {}", pin.name))
    }

    /// `ANL C, /pin`
    pub fn anl_c_not_pin(&mut self, pin: Bit) -> &mut Self {
        self.raw(&format!("ANL   C, /{}", pin.name))
    }

    /// `ORL C, pin`
    pub fn orl_c_pin(&mut self, pin: Bit) -> &mut Self {
        self.raw(&format!("ORL   C, {}", pin.name))
    }

    /// `ORL C, /pin`
    pub fn orl_c_not_pin(&mut self, pin: Bit) -> &mut Self {
        self.raw(&format!("ORL   C, /{}", pin.name))
    }

    /// `MOV C, pin`
    pub fn mov_c_pin(&mut self, pin: Bit) -> &mut Self {
        self.raw(&format!("MOV   C, {}", pin.name))
    }

    /// `MOV pin, C`
    pub fn mov_pin_c(&mut self, pin: Bit) -> &mut Self {
        self.raw(&format!("MOV   {}, C", pin.name))
    }

    // -----------------------------------------------------------------------
    // 调用与返回
    // -----------------------------------------------------------------------

    /// 调用一个目标机子程序（[`Routine`]）。
    ///
    /// 参数怎么传由子程序的约定决定（常见做法是 A / R7 / R6 …… 依次放参数）。
    /// 用 [`Task::push_reg`] / [`Task::pop_reg`] 做被调用者保存。
    pub fn call_routine(&mut self, routine: &Routine) -> &mut Self {
        let symbol = routine.symbol.clone();
        self.call(&symbol)
    }

    /// 尾调用：`LJMP` 跳到子程序，由它的 `RET` 返回到**本函数的调用者**。
    /// 适合状态机之类的尾递归写法，可以省一层栈。
    pub fn tail_call(&mut self, routine: &Routine) -> &mut Self {
        let symbol = routine.symbol.clone();
        self.jump_to(&symbol)
    }

    /// `LCALL symbol` —— 调用一个标签或内核例程。
    ///
    /// 这里固定用 `LCALL` 而不是 `ACALL`：`ACALL` 只能跳到 2KB 页内，
    /// 跨到内核里去很容易越界。
    pub fn call(&mut self, symbol: &str) -> &mut Self {
        self.raw(&format!("LCALL {symbol}"))
    }

    /// `LJMP symbol`
    pub fn jump_to(&mut self, symbol: &str) -> &mut Self {
        self.raw(&format!("LJMP  {symbol}"))
    }

    /// `RET`
    pub fn ret(&mut self) -> &mut Self {
        self.raw("RET")
    }

    /// `RETI`
    pub fn reti(&mut self) -> &mut Self {
        self.raw("RETI")
    }

    /// `NOP`
    pub fn nop(&mut self) -> &mut Self {
        self.raw("NOP")
    }

    /// 往一个直接地址（SFR）写值。
    pub fn store(&mut self, direct: impl Into<u8>, value: u8) -> &mut Self {
        self.mov_direct_imm(direct, value)
    }

    // -----------------------------------------------------------------------
    // 常用的小工具（由 codegen 在 CODE 段里生成一份实现）
    // -----------------------------------------------------------------------

    /// 把一段文本注册成 CODE 段里的 NUL 结尾常量，返回它的标签。
    ///
    /// 标签由内容哈希决定，所以同一段文本出现多次只会生成一份常量。
    /// 配合 `MOV DPTR, #<label>` + `MOVC A, @A+DPTR` 就能读回来 ——
    /// 这是 8051 上访问只读数据的标准做法。
    pub fn string_label(&mut self, text: &str) -> String {
        let bytes: Vec<u8> = text
            .as_bytes()
            .iter()
            .copied()
            .chain(core::iter::once(0))
            .collect();
        let key = fnv1a(&bytes);
        self.strings.push(bytes);
        format!("__str_{key:08X}")
    }

    /// 忙等 `ms` 毫秒。**入口：`A` 不能再被占用** —— 这个方法自己会写 `A`。
    ///
    /// 生成的是 `__delay_ms` 子程序：三层空循环，按 11.0592MHz 标定
    /// （1ms ≈ 921 个机器周期）。它只保存/恢复 `R1`、`R2`，会破坏 `PSW`。
    ///
    /// 单次上限 255ms —— 要更久就多调几次。换算全在主机上做完，
    /// 目标机只做减法。
    pub fn delay_ms(&mut self, ms: u32) -> &mut Self {
        if ms > 255 {
            self.fail(format!(
                "delay_ms({ms}) 超过单次上限 255ms；拆成多次调用即可"
            ));
            return self;
        }
        self.need(Helper::DelayMs);
        self.raw_commented(&format!("忙等 {ms}ms"), &format!("MOV   A, #{ms}"));
        self.call("__delay_ms")
    }
}
