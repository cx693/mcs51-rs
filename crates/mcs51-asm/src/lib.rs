//! # mcs51-asm
//!
//! A relocatable assembler for the Intel MCS-51 (8051), written in Rust.
//!
//! This is the `sdas8051` of the `mcs51-rs` toolchain. It consumes 8051 assembly
//! source and produces an [`ObjectFile`]: one or more named sections of bytes,
//! each with relocations into a symbol table that [`mcs51_ld`] resolves.
//!
//! ## Syntax
//!
//! ```asm
//! ; comment -- ';' to end of line, or '//'
//!         .area   CODE (ABS)
//!         .org    0x0000
//! RESET:  LJMP    start
//!         .org    0x000B
//!         LJMP    timer0_isr
//!
//!         .area   CODE
//! start:  MOV     SP, #0x30
//!         MOV     DPTR, #message
//! loop:   CLR     A
//!         MOVC    A, @A+DPTR
//!         JZ      done
//!         INC     DPTR
//!         SJMP    loop
//! done:   SJMP    done
//!
//! message: .db    "hello", 0x0A, 0
//! count:   .equ    5
//! flag:    .bit    TR0
//!         .end
//! ```
//!
//! Directives: `.area`, `.org`, `.equ`/`.set`, `.bit`, `.db`/`.byte`, `.dw`/
//! `.word`, `.ds`/`.space`, `.align`, `.globl`, `.extern`, `.include`, `.end`.
//!
//! Expressions use `+ - * / % << >> & | ^ ~` and parentheses, the current
//! location `$`, character literals `'A'`, and the `low()`/`high()` functions.
//!
//! ## Example
//!
//! ```
//! use mcs51_asm::{assemble, Options};
//!
//! let source = "
//!         .area CODE (ABS)
//!         .org  0x0100
//! start:  MOV   A, #0x42
//!         SJMP  start
//! ";
//! let out = assemble(source, &Options::new("demo.asm"));
//! assert!(out.diagnostics.is_empty(), "{:?}", out.diagnostics);
//! assert_eq!(out.object.section("CODE").unwrap().data, vec![0x74, 0x42, 0x80, 0xFC]);
//! ```

use std::fmt;

pub use mcs51_isa::mem::Space;

mod asm;
mod expr;
mod object;
mod parser;

pub use expr::{Expr, Value};
pub use object::{ObjectFile, Reloc, RelocKind, Section, Symbol, SymbolKind, SymbolLoc};
pub use parser::{OperandText, ParsedLine, Statement};

/// Where an assembly unit's include files are looked up, and what is predefined.
#[derive(Debug, Clone)]
pub struct Options {
    /// Name reported in diagnostics, normally the source path.
    pub filename: String,
    /// Directories searched by `.include`.
    pub include_paths: Vec<std::path::PathBuf>,
    /// Symbols defined before assembly starts (`.equ`-like, absolute values).
    pub defines: Vec<(String, i64)>,
    /// Predefine the standard 8051 SFR, bit and mode-constant names.
    ///
    /// On by default, so `MOV SP, #0x30` and `SETB TR0` work without an
    /// explicit `.include "8051.inc"`.
    pub builtin_symbols: bool,
    /// Include files supplied in memory, as `(name, contents)`.
    ///
    /// Checked before the filesystem, which is how the kernel build injects a
    /// generated configuration header without touching the disk.
    pub virtual_includes: Vec<(String, String)>,
    /// Emit a listing alongside the object.
    pub listing: bool,
    /// Maximum number of assembly passes before giving up on symbol resolution.
    pub max_passes: u32,
}

impl Default for Options {
    fn default() -> Self {
        Self::new("<input>")
    }
}

impl Options {
    /// Options for a named source file.
    pub fn new(filename: impl Into<String>) -> Self {
        Self {
            filename: filename.into(),
            include_paths: Vec::new(),
            defines: Vec::new(),
            builtin_symbols: true,
            virtual_includes: Vec::new(),
            listing: true,
            max_passes: 5,
        }
    }

    /// Add an include search directory.
    pub fn include(mut self, path: impl Into<std::path::PathBuf>) -> Self {
        self.include_paths.push(path.into());
        self
    }

    /// Predefine an absolute symbol.
    pub fn define(mut self, name: impl Into<String>, value: i64) -> Self {
        self.defines.push((name.into(), value));
        self
    }

    /// Add an in-memory include file, which takes priority over the filesystem.
    pub fn with_virtual_include(
        mut self,
        name: impl Into<String>,
        contents: impl Into<String>,
    ) -> Self {
        self.virtual_includes.push((name.into(), contents.into()));
        self
    }
}

/// Severity of a [`Diagnostic`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    /// Something suspicious that did not stop assembly.
    Warning,
    /// Assembly of the unit failed.
    Error,
}

impl Severity {
    /// Lower-case name, as printed.
    pub const fn as_str(self) -> &'static str {
        match self {
            Severity::Warning => "warning",
            Severity::Error => "error",
        }
    }
}

impl fmt::Display for Severity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One assembler message, with source position.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    /// Source file the message refers to.
    pub file: String,
    /// 1-based line number.
    pub line: usize,
    /// 0-based column, or `None` when the whole line is at fault.
    pub column: Option<usize>,
    /// How serious it is.
    pub severity: Severity,
    /// What went wrong.
    pub message: String,
    /// The offending source line, for context.
    pub source: Option<String>,
}

impl Diagnostic {
    /// A new error.
    pub fn error(file: impl Into<String>, line: usize, message: impl Into<String>) -> Self {
        Self {
            file: file.into(),
            line,
            column: None,
            severity: Severity::Error,
            message: message.into(),
            source: None,
        }
    }

    /// A new warning.
    pub fn warning(file: impl Into<String>, line: usize, message: impl Into<String>) -> Self {
        Self {
            severity: Severity::Warning,
            ..Self::error(file, line, message)
        }
    }

    /// Attach the source line and a column.
    pub fn at(mut self, column: usize, source: impl Into<String>) -> Self {
        self.column = Some(column);
        self.source = Some(source.into());
        self
    }
}

impl fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.column {
            Some(col) => write!(
                f,
                "{}:{}:{}: {}: {}",
                self.file,
                self.line,
                col + 1,
                self.severity,
                self.message
            )?,
            None => write!(
                f,
                "{}:{}: {}: {}",
                self.file, self.line, self.severity, self.message
            )?,
        }
        if let Some(src) = &self.source {
            write!(f, "\n  {}\n  ", src.trim_end())?;
            let col = self.column.unwrap_or(0).min(src.trim_end().len());
            for _ in 0..col {
                f.write_str(" ")?;
            }
            f.write_str("^")?;
        }
        Ok(())
    }
}

/// One line of an assembly listing.
#[derive(Debug, Clone)]
pub struct ListingLine {
    /// Address the line assembled at, if it produced code.
    pub addr: Option<u16>,
    /// Encoded bytes, if any.
    pub bytes: Vec<u8>,
    /// 1-based source line number.
    pub line: usize,
    /// The original source text.
    pub text: String,
    /// Symbols defined on this line.
    pub symbols: Vec<String>,
}

/// The result of assembling one source unit.
#[derive(Debug, Clone)]
pub struct AssembleOutput {
    /// The relocatable object, ready for the linker.
    pub object: ObjectFile,
    /// Per-line listing, when [`Options::listing`] is set.
    pub listing: Vec<ListingLine>,
    /// Warnings and errors, in source order.
    pub diagnostics: Vec<Diagnostic>,
}

impl AssembleOutput {
    /// `true` if assembly produced no errors.
    pub fn is_ok(&self) -> bool {
        !self
            .diagnostics
            .iter()
            .any(|d| d.severity == Severity::Error)
    }

    /// Just the errors.
    pub fn errors(&self) -> impl Iterator<Item = &Diagnostic> {
        self.diagnostics
            .iter()
            .filter(|d| d.severity == Severity::Error)
    }

    /// All diagnostics rendered one per line.
    pub fn render_diagnostics(&self) -> String {
        self.diagnostics
            .iter()
            .map(|d| d.to_string())
            .collect::<Vec<_>>()
            .join("\n")
    }
}

/// Assemble a source unit.
///
/// Assembly never panics on bad input: everything the assembler dislikes becomes
/// a [`Diagnostic`]. A return value with errors may still carry a partial
/// object, which is useful for editor tooling but must not be linked.
pub fn assemble(source: &str, options: &Options) -> AssembleOutput {
    asm::run(source, options)
}

/// Convenience: assemble a file from disk.
pub fn assemble_file(path: impl AsRef<std::path::Path>) -> std::io::Result<AssembleOutput> {
    let path = path.as_ref();
    let text = std::fs::read_to_string(path)?;
    let mut options = Options::new(path.display().to_string());
    if let Some(dir) = path.parent() {
        options.include_paths.push(dir.to_path_buf());
    }
    options.include_paths.push(std::path::PathBuf::from("."));
    Ok(assemble(&text, &options))
}

/// Standard 8051 device header, spliced in when a source asks for `.include
/// "8051.inc"` and no such file is found on disk.
///
/// Keeps the toolchain self-contained: an assembly file can name every SFR and
/// bit it needs without shipping a header file alongside it.
pub const BUILTIN_8051_INC: &str = include_str!("../include/8051.inc");

/// The built-in headers, by file name.
pub fn builtin_include(name: &str) -> Option<&'static str> {
    match name {
        "8051.inc" | "mcs51.inc" | "8051.def" => Some(BUILTIN_8051_INC),
        _ => None,
    }
}

/// The symbols the assembler knows without being told: every named SFR, every
/// named bit, the port-bit aliases and the standard mode constants.
/// 把一个符号放进表里；重名的跳过，所以**先放的优先**。
fn push_symbol(
    out: &mut Vec<(String, i64, SymbolKind)>,
    name: String,
    value: i64,
    kind: SymbolKind,
) {
    if !out.iter().any(|(existing, _, _)| *existing == name) {
        out.push((name, value, kind));
    }
}

pub fn builtin_symbols() -> Vec<(String, i64, SymbolKind)> {
    let mut out: Vec<(String, i64, SymbolKind)> = Vec::new();

    // ---- 1) 芯片标准库优先 ----
    //
    // 这是**地址的权威来源**：Keil `reg52.h` / STC 头文件的 Rust 版。
    // 它必须排在前面 —— 否则下面 `mcs51-isa` 里同名的条目会把它遮住，
    // 于是"地址只有一个来源"就成了空话（这个坑真的踩过：
    // 把芯片库里的 P35 改错，生成的固件毫无变化）。
    //
    // 用 STC89C5xRC 的合并视图：它继承了 reg52，基础和扩展一次全覆盖。
    for chip in mcs51_chips::CHIPS {
        for sfr in chip.all_sfr() {
            push_symbol(
                &mut out,
                sfr.name.to_string(),
                sfr.addr as i64,
                SymbolKind::Sfr,
            );
        }
        for bit in chip.all_bits() {
            push_symbol(
                &mut out,
                bit.name.to_string(),
                bit.addr as i64,
                SymbolKind::Bit,
            );
        }
    }
    for sfr in mcs51_chips::reg52::SFR {
        push_symbol(
            &mut out,
            sfr.name.to_string(),
            sfr.addr as i64,
            SymbolKind::Sfr,
        );
    }
    for bit in mcs51_chips::reg52::BITS {
        push_symbol(
            &mut out,
            bit.name.to_string(),
            bit.addr as i64,
            SymbolKind::Bit,
        );
    }

    // ---- 2) eDSL 习惯的下划线引脚名（P3_5）----
    //
    // 地址**按名字向芯片库要**，不自己算 `端口基地址 + 序号`。
    // 自己算等于把"P3.5 是第 5 位"这件事写了两遍 —— 芯片库里改一次，
    // 这里不跟着变，两份定义就漂移了（这个坑也真的踩过）。
    for port in 0..4u8 {
        for bit in 0..8u8 {
            let keil_name = format!("P{port}{bit}");
            let pin = mcs51_chips::reg52::CHIP
                .bit_named(&keil_name)
                .unwrap_or_else(|| panic!("芯片库里应该有 {keil_name}"));
            push_symbol(
                &mut out,
                format!("P{port}_{bit}"),
                pin.addr as i64,
                SymbolKind::Bit,
            );
        }
    }

    // ---- 3) mcs51-isa 的表作为兜底 ----
    //
    // 补芯片库没有的名字（通用 8051 的杂项符号）。同名的会被上面的挡住，
    // 这正是我们要的：地址以芯片库为准。
    for (name, addr) in mcs51_isa::sfr::SFR_NAMES {
        push_symbol(&mut out, (*name).to_string(), *addr as i64, SymbolKind::Sfr);
    }
    for (name, addr) in mcs51_isa::sfr::BIT_NAMES {
        // `C/T2` style names are documentation, not identifiers.
        if !name.contains('/') {
            push_symbol(&mut out, (*name).to_string(), *addr as i64, SymbolKind::Bit);
        }
    }

    // ---- 4) 位掩码常量（不是地址，谁先谁后无所谓）----
    let constants: &[(&str, i64)] = &[
        ("IE_EA", 0x80),
        ("IE_ES", 0x10),
        ("IE_ET1", 0x08),
        ("IE_EX1", 0x04),
        ("IE_ET0", 0x02),
        ("IE_EX0", 0x01),
        ("TMOD_T1_GATE", 0x80),
        ("TMOD_T1_CT", 0x40),
        ("TMOD_T1_MODE1", 0x10),
        ("TMOD_T0_GATE", 0x08),
        ("TMOD_T0_CT", 0x04),
        ("TMOD_T0_MODE2", 0x02),
        ("TMOD_T0_TIMER_MODE0", 0x00),
        ("TMOD_T0_TIMER_MODE1", 0x01),
        ("TMOD_T0_TIMER_MODE2", 0x02),
        ("TMOD_T0_TIMER_MODE3", 0x03),
        ("PCON_IDL", 0x01),
        ("PCON_PD", 0x02),
        ("SCON_MODE0", 0x00),
        ("SCON_MODE1", 0x40),
        ("SCON_MODE2", 0x80),
        ("SCON_MODE3", 0xC0),
    ];
    for (name, value) in constants {
        push_symbol(&mut out, (*name).to_string(), *value, SymbolKind::Equate);
    }

    out
}

/// Section names used by the standard linker scripts.
pub mod sections {
    /// Interrupt vectors and reset code, forced to the bottom of code space.
    pub const HOME: &str = "HOME";
    /// Ordinary program code.
    pub const CODE: &str = "CODE";
    /// Initialised internal data (copied to RAM at startup).
    pub const DATA: &str = "DATA";
    /// Uninitialised internal data.
    pub const BSS: &str = "BSS";
    /// Initialised external data.
    pub const XDATA: &str = "XDATA";
    /// Bit variables.
    pub const BIT: &str = "BIT";
}
