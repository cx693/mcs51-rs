//! The two-phase assembler.

use std::collections::{BTreeMap, BTreeSet};

use mcs51_isa::Enc;
use mcs51_isa::mem::Space;
use mcs51_isa::{Arg, EncodeError, Instruction, Mnemonic, Operand, OperandForm, encode};

use crate::expr::{Expr, ExprError, Resolved, Transform, Value};
use crate::object::{ObjectFile, Reloc, RelocKind, Section, Symbol, SymbolKind, SymbolLoc};
use crate::parser::{self, DataItem, OperandText, ParsedLine, Statement};
use crate::{AssembleOutput, Diagnostic, ListingLine, Options};

/// Per-section state while assembling.
struct SectionState {
    name: String,
    space: Space,
    /// Absolute base address, once known.
    org: Option<u16>,
    /// Whether `.area` asked for an absolute section.
    wants_absolute: bool,
    /// Whether the base came from `.org` (rather than being assigned later).
    org_from_directive: bool,
    size: u32,
    data: Vec<u8>,
    relocs: Vec<Reloc>,
}

impl SectionState {
    fn new(name: String, space: Space, absolute: bool) -> Self {
        Self {
            name,
            space,
            org: None,
            wants_absolute: absolute,
            org_from_directive: false,
            size: 0,
            data: Vec::new(),
            relocs: Vec::new(),
        }
    }
}

/// A symbol as the assembler knows it, before the linker sees it.
#[derive(Clone)]
struct Def {
    kind: SymbolKind,
    loc: SymbolLoc,
    file: String,
    line: usize,
    /// The expression for a deferred `.equ`.
    pending: Option<Expr>,
}

struct Asm<'a> {
    options: &'a Options,
    lines: Vec<ParsedLine>,
    sections: Vec<SectionState>,
    section_index: BTreeMap<String, usize>,
    symbols: BTreeMap<String, Def>,
    globals: BTreeSet<String>,
    externs: BTreeSet<String>,
    entry: Option<String>,
    diagnostics: Vec<Diagnostic>,
    listing: Vec<ListingLine>,
    /// Section and offset assigned to each line by the sizing phase.
    placement: Vec<Option<(usize, u32, u32)>>,
}

/// Assemble a source unit.
pub(crate) fn run(source: &str, options: &Options) -> AssembleOutput {
    let mut diagnostics = Vec::new();
    let lines = parser::parse_source(source, &options.filename, options, &mut diagnostics);

    let mut asm = Asm {
        options,
        placement: vec![None; lines.len()],
        lines,
        sections: Vec::new(),
        section_index: BTreeMap::new(),
        symbols: BTreeMap::new(),
        globals: BTreeSet::new(),
        externs: BTreeSet::new(),
        entry: None,
        diagnostics,
        listing: Vec::new(),
    };

    if options.builtin_symbols {
        for (name, value, kind) in crate::builtin_symbols() {
            asm.symbols.insert(
                name,
                Def {
                    kind,
                    loc: SymbolLoc::Absolute(value),
                    file: "<builtin>".into(),
                    line: 0,
                    pending: None,
                },
            );
        }
    }

    for (name, value) in &options.defines {
        asm.symbols.insert(
            name.clone(),
            Def {
                kind: SymbolKind::Equate,
                loc: SymbolLoc::Absolute(*value),
                file: "<command line>".into(),
                line: 0,
                pending: None,
            },
        );
    }

    // Ensure a default section exists so that a source with no `.area` works.
    asm.section_or_insert("CODE", Space::Code, false);

    asm.size_all();
    asm.emit_all();
    asm.finish()
}

impl<'a> Asm<'a> {
    fn error(&mut self, line: usize, message: impl Into<String>) {
        self.error_in(&self.options.filename.clone(), line, message);
    }

    fn error_in(&mut self, file: &str, line: usize, message: impl Into<String>) {
        let source = self
            .lines
            .iter()
            .find(|l| l.file == file && l.line == line)
            .map(|l| l.text.clone());
        let mut diagnostic = Diagnostic::error(file, line, message);
        if let Some(source) = source {
            diagnostic = diagnostic.at(parser::strip_comment(&source).len(), source);
        }
        self.diagnostics.push(diagnostic);
    }

    fn section_or_insert(&mut self, name: &str, space: Space, absolute: bool) -> usize {
        if let Some(index) = self.section_index.get(name) {
            return *index;
        }
        let index = self.sections.len();
        self.sections
            .push(SectionState::new(name.to_string(), space, absolute));
        self.section_index.insert(name.to_string(), index);
        index
    }

    fn resolve(&self, name: &str) -> Option<Resolved> {
        let symbol = self.symbols.get(name)?;
        match &symbol.loc {
            SymbolLoc::Absolute(value) => Some(Resolved::Absolute(*value)),
            SymbolLoc::Section { section, offset } => match self.section_index.get(section) {
                Some(index) => match self.sections[*index].org {
                    Some(org) => Some(Resolved::Located(org as i64 + *offset as i64)),
                    None => Some(Resolved::Relocatable {
                        section: section.clone(),
                        offset: *offset,
                    }),
                },
                None => Some(Resolved::Relocatable {
                    section: section.clone(),
                    offset: *offset,
                }),
            },
        }
    }

    /// Evaluate an expression, promoting undefined symbols to external
    /// references so the linker can resolve them.
    fn eval_operand(
        &mut self,
        expr: &Expr,
        here: i64,
    ) -> Result<Value, (ExprError, Option<String>)> {
        if let Ok(value) = expr.eval(&|n| self.resolve(n), here) {
            return Ok(value);
        }
        let external: std::cell::Cell<Option<String>> = std::cell::Cell::new(None);
        let symbols = &self.symbols;
        let sections = &self.sections;
        let index = &self.section_index;
        let lookup = |name: &str| -> Option<Resolved> {
            if let Some(symbol) = symbols.get(name) {
                return match &symbol.loc {
                    SymbolLoc::Absolute(value) => Some(Resolved::Absolute(*value)),
                    SymbolLoc::Section { section, offset } => match index.get(section) {
                        Some(i) => match sections[*i].org {
                            Some(org) => Some(Resolved::Located(org as i64 + *offset as i64)),
                            None => Some(Resolved::Relocatable {
                                section: section.clone(),
                                offset: *offset,
                            }),
                        },
                        None => Some(Resolved::Relocatable {
                            section: section.clone(),
                            offset: *offset,
                        }),
                    },
                };
            }
            external.set(Some(name.to_string()));
            Some(Resolved::Relocatable {
                section: String::new(),
                offset: 0,
            })
        };
        match expr.eval(&lookup, here) {
            Ok(value) => {
                if let Some(name) = external.take() {
                    self.externs.insert(name);
                }
                Ok(value)
            }
            Err(e) => Err((e, external.take())),
        }
    }

    // ---------------------------------------------------------------
    // Phase 1: assign addresses and sizes
    // ---------------------------------------------------------------

    fn size_all(&mut self) {
        let mut pass = 0;
        loop {
            pass += 1;
            let before_symbols = self.symbols.len();
            let before_sizes: Vec<u32> = self.sections.iter().map(|s| s.size).collect();

            for section in &mut self.sections {
                section.size = 0;
            }
            self.placement.iter_mut().for_each(|p| *p = None);

            let mut current = self
                .section_index
                .get("HOME")
                .or_else(|| self.section_index.get("CODE"))
                .copied()
                .unwrap_or(0);
            let mut offset: u32 = 0;

            for index in 0..self.lines.len() {
                let (next_section, next_offset) = self.size_line(index, current, offset);
                current = next_section;
                offset = next_offset;
            }

            let after_sizes: Vec<u32> = self.sections.iter().map(|s| s.size).collect();
            let stable =
                before_symbols == self.symbols.len() && before_sizes == after_sizes && pass > 1;
            if stable || pass >= self.options.max_passes {
                if !stable && pass >= self.options.max_passes {
                    self.diagnostics.push(Diagnostic::warning(
                        &self.options.filename,
                        0,
                        format!("assembly did not converge after {pass} passes"),
                    ));
                }
                break;
            }
        }

        for section in &mut self.sections {
            section.data = vec![0u8; section.size as usize];
        }
    }

    fn size_line(&mut self, index: usize, mut current: usize, mut offset: u32) -> (usize, u32) {
        let line = self.lines[index].clone();

        if let Some(name) = &line.label {
            self.define_label(name, current, offset, &line);
        }

        let here = self.abs_here(current, offset);
        let mut advance = 0u32;

        match &line.statement {
            Statement::Empty | Statement::End => {}
            Statement::Instruction { mnemonic, operands } => {
                advance = self.size_instruction(*mnemonic, operands);
            }
            Statement::Data { width, items } => {
                for item in items {
                    advance += match item {
                        DataItem::Text(bytes) => bytes.len() as u32,
                        DataItem::Value(_) => *width as u32,
                    };
                }
            }
            Statement::Reserve { count, fill: _ } => match count.eval(&|n| self.resolve(n), here) {
                Ok(value) => match value.as_abs() {
                    Some(n) if n >= 0 => advance = n as u32,
                    Some(n) => self.error(line.line, format!("negative .ds size {n}")),
                    None => self.error(line.line, ".ds size must be a constant"),
                },
                Err(e) => self.error(line.line, format!(".ds: {e}")),
            },
            Statement::Org(target) => match target.eval(&|n| self.resolve(n), here) {
                Ok(value) => match value.as_abs() {
                    Some(addr) if (0..=0xFFFF).contains(&addr) => {
                        let addr = addr as u32;
                        if self.sections[current].org.is_none() {
                            self.sections[current].org = Some(addr as u16);
                            self.sections[current].org_from_directive = true;
                            offset = 0;
                        }
                        let base = self.sections[current].org.unwrap() as u32;
                        if addr < base || addr < base + offset {
                            self.error(
                                    line.line,
                                    format!(
                                        ".org {addr:#06X} moves backwards (area base {base:#06X},                                          {} byte(s) already emitted)",
                                        offset
                                    ),
                                );
                        } else {
                            advance = addr - base - offset;
                        }
                    }
                    Some(addr) => self.error(line.line, format!(".org {addr} out of range")),
                    None => self.error(line.line, ".org must be a constant"),
                },
                Err(e) => self.error(line.line, format!(".org: {e}")),
            },
            Statement::Align(align) => match align.eval(&|n| self.resolve(n), here) {
                Ok(value) => match value.as_abs() {
                    Some(n) if n > 0 && (n as u64).is_power_of_two() => {
                        let base = self.sections[current].org.map(|o| o as u32).unwrap_or(0);
                        let at = (base + offset + advance) as u64;
                        let n = n as u64;
                        advance += ((n - (at % n)) % n) as u32;
                    }
                    Some(n) => self.error(line.line, format!(".align {n} must be a power of two")),
                    None => self.error(line.line, ".align must be a constant"),
                },
                Err(e) => self.error(line.line, format!(".align: {e}")),
            },
            Statement::Area {
                name,
                space,
                absolute,
            } => {
                current = self.section_or_insert(name, *space, *absolute);
                // Re-entering an area resumes where it left off; only a brand
                // new area starts at zero.
                offset = self.sections[current].size;
                advance = 0;
            }
            Statement::Define { name, expr, kind } => {
                self.define_equate(name, expr, *kind, &line);
            }
            Statement::Globals(names) => {
                for name in names {
                    self.globals.insert(name.clone());
                }
            }
            Statement::Externs(names) => {
                for name in names {
                    self.externs.insert(name.clone());
                }
            }
            Statement::Entry(name) => self.entry = Some(name.clone()),
            Statement::Unknown { .. } => {}
        }

        offset += advance;
        if offset > self.sections[current].size {
            self.sections[current].size = offset;
        }
        self.placement[index] = Some((current, offset - advance, advance));
        (current, offset)
    }

    fn abs_here(&self, section: usize, offset: u32) -> i64 {
        match self.sections[section].org {
            Some(org) => org as i64 + offset as i64,
            None => offset as i64,
        }
    }

    fn size_instruction(&mut self, mnemonic: Mnemonic, operands: &[OperandText]) -> u32 {
        let forms = match self.dummy_forms(operands) {
            Some(forms) => forms,
            None => return 1,
        };
        match Instruction::select(0, mnemonic, &forms) {
            Ok((_, def)) => def.len() as u32,
            Err(_) => 1,
        }
    }

    /// Operand forms with placeholder values, used purely to pick the opcode.
    fn dummy_forms(&self, operands: &[OperandText]) -> Option<Vec<OperandForm>> {
        operands
            .iter()
            .map(|operand| to_form(operand, Some(0)))
            .collect()
    }

    fn define_label(&mut self, name: &str, section: usize, offset: u32, line: &ParsedLine) {
        let section_name = self.sections[section].name.clone();
        self.symbols.insert(
            name.to_string(),
            Def {
                kind: SymbolKind::Label,
                loc: SymbolLoc::Section {
                    section: section_name,
                    offset,
                },
                file: line.file.clone(),
                line: line.line,
                pending: None,
            },
        );
    }

    fn define_equate(&mut self, name: &str, expr: &Expr, kind: SymbolKind, line: &ParsedLine) {
        let here = 0i64;
        let value = match expr.eval(&|n| self.resolve(n), here) {
            Ok(value) => value,
            Err(ExprError::UndefinedSymbol(_)) => {
                // Defer: a later pass may have the symbol.
                self.symbols.insert(
                    name.to_string(),
                    Def {
                        kind,
                        loc: SymbolLoc::Absolute(0),
                        file: line.file.clone(),
                        line: line.line,
                        pending: Some(expr.clone()),
                    },
                );
                return;
            }
            Err(e) => {
                self.error_in(&line.file, line.line, format!("`{name}`: {e}"));
                return;
            }
        };
        let loc = match value {
            Value::Abs(v) => SymbolLoc::Absolute(v),
            Value::Reloc {
                symbol,
                addend,
                section,
                offset,
                transform: Transform::Plain,
            } => match (section, offset) {
                (Some(section), Some(offset)) if addend >= 0 => SymbolLoc::Section {
                    section,
                    offset: offset + addend as u32,
                },
                _ => {
                    self.error_in(
                        &line.file,
                        line.line,
                        format!("`{name}` aliases `{symbol}` and cannot be reduced to an address"),
                    );
                    return;
                }
            },
            Value::Reloc { symbol, .. } => {
                self.error_in(
                    &line.file,
                    line.line,
                    format!("`{name}` uses low()/high() of `{symbol}` and is not a valid equate"),
                );
                return;
            }
        };
        self.symbols.insert(
            name.to_string(),
            Def {
                kind,
                loc,
                file: line.file.clone(),
                line: line.line,
                pending: None,
            },
        );
    }

    // ---------------------------------------------------------------
    // Phase 2: emit bytes
    // ---------------------------------------------------------------

    fn emit_all(&mut self) {
        let lines = self.lines.clone();
        for (index, line) in lines.iter().enumerate() {
            let Some((section, offset, size)) = self.placement[index] else {
                continue;
            };
            let mut bytes = Vec::new();
            match &line.statement {
                Statement::Instruction { mnemonic, operands } => {
                    bytes = self.emit_instruction(line, section, offset, *mnemonic, operands);
                }
                Statement::Data { width, items } => {
                    bytes = self.emit_data(line, section, offset, *width, items);
                }
                Statement::Reserve { count, fill } => {
                    let here = self.abs_here(section, offset);
                    let n = count
                        .eval(&|name| self.resolve(name), here)
                        .ok()
                        .and_then(|v| v.as_abs())
                        .unwrap_or(0)
                        .max(0) as u32;
                    let fill_byte = fill
                        .as_ref()
                        .and_then(|f| f.eval(&|name| self.resolve(name), here).ok())
                        .and_then(|v| v.as_abs())
                        .unwrap_or(0) as u8;
                    bytes = vec![fill_byte; n as usize];
                }
                Statement::Org(_) => {
                    // Padding was reserved by the sizing pass; fill it.
                    let size = size as usize;
                    if size > 0 {
                        bytes = vec![0xFF; size];
                    }
                }
                _ => {}
            }

            if !bytes.is_empty() {
                let data = &mut self.sections[section].data;
                let start = offset as usize;
                let end = (start + bytes.len()).min(data.len());
                data[start..end].copy_from_slice(&bytes[..end - start]);
            }

            let mut symbols = Vec::new();
            if let Some(name) = &line.label {
                symbols.push(name.clone());
            }
            self.listing.push(ListingLine {
                addr: self.sections[section].org.map(|o| o + offset as u16),
                bytes,
                line: line.line,
                text: line.text.clone(),
                symbols,
            });
        }

        // Anything still pending gets one last chance now that the sizing pass
        // has defined every label.
        let pending: Vec<(String, Expr, SymbolKind, String, usize)> = self
            .symbols
            .iter()
            .filter_map(|(name, def)| {
                def.pending.as_ref().map(|expr| {
                    (
                        name.clone(),
                        expr.clone(),
                        def.kind,
                        def.file.clone(),
                        def.line,
                    )
                })
            })
            .collect();
        for (name, expr, _kind, file, line) in pending {
            let here = 0i64;
            match expr.eval(&|n| self.resolve(n), here) {
                Ok(Value::Abs(v)) => {
                    if let Some(def) = self.symbols.get_mut(&name) {
                        def.loc = SymbolLoc::Absolute(v);
                        def.pending = None;
                    }
                }
                Ok(Value::Reloc {
                    section,
                    offset,
                    addend,
                    transform: Transform::Plain,
                    ..
                }) => {
                    if let (Some(section), Some(offset)) = (section, offset)
                        && let Some(def) = self.symbols.get_mut(&name)
                    {
                        def.loc = SymbolLoc::Section {
                            section,
                            offset: offset + addend.max(0) as u32,
                        };
                        def.pending = None;
                    }
                }
                Ok(other) => {
                    self.error_in(&file, line, format!("`{name}` is not a constant: {other}"))
                }
                Err(e) => self.error_in(&file, line, format!("`{name}`: {e}")),
            }
        }
    }

    fn emit_instruction(
        &mut self,
        line: &ParsedLine,
        section: usize,
        offset: u32,
        mnemonic: Mnemonic,
        operands: &[OperandText],
    ) -> Vec<u8> {
        let here = self.abs_here(section, offset);
        let mut values = Vec::with_capacity(operands.len());
        for operand in operands {
            let expr = match operand {
                OperandText::Imm(e) | OperandText::NotBit(e) | OperandText::Bare(e) => Some(e),
                OperandText::Bad(text) => {
                    self.error_in(&line.file, line.line, format!("bad operand `{text}`"));
                    return Vec::new();
                }
                _ => None,
            };
            match expr {
                Some(expr) => match self.eval_operand(expr, here) {
                    Ok(value) => values.push(Some(value)),
                    Err((e, _)) => {
                        self.error_in(&line.file, line.line, format!("{e}"));
                        return Vec::new();
                    }
                },
                None => values.push(None),
            }
        }

        let forms: Vec<OperandForm> = operands
            .iter()
            .zip(&values)
            .filter_map(|(operand, value)| {
                let number = match value {
                    Some(Value::Abs(v)) => Some(*v as u32 & 0xFFFF),
                    Some(Value::Reloc { .. }) => Some(0),
                    None => None,
                };
                to_form(operand, number)
            })
            .collect();
        if forms.len() != operands.len() {
            self.error_in(&line.file, line.line, "unsupported operand form");
            return Vec::new();
        }

        let addr = self.sections[section]
            .org
            .map(|o| o.wrapping_add(offset as u16));
        let (mut instruction, def) = match Instruction::select(addr.unwrap_or(0), mnemonic, &forms)
        {
            Ok(pair) => pair,
            Err(e) => {
                self.error_in(&line.file, line.line, format!("{e}"));
                return Vec::new();
            }
        };

        for (i, arg) in def.args.iter().enumerate() {
            let Some(Some(value)) = values.get(i).cloned() else {
                continue;
            };
            let Value::Reloc {
                symbol,
                addend,
                transform,
                section: section_of,
                offset: offset_of,
            } = value
            else {
                continue;
            };
            let kind = match reloc_kind(*arg, transform) {
                Ok(kind) => kind,
                Err(message) => {
                    self.error_in(&line.file, line.line, message);
                    return Vec::new();
                }
            };

            // A branch within this section never needs the linker: the
            // displacement is the same wherever the section lands.
            if *arg == Arg::Rel
                && let Some(target_section) = &section_of
                && let Some(target_offset) = offset_of
                && *target_section == self.sections[section].name
            {
                let target = target_offset as i64 + addend;
                let next = offset as i64 + def.len() as i64;
                let delta = target - next;
                if !(-128..=127).contains(&delta) {
                    self.error_in(
                        &line.file,
                        line.line,
                        format!(
                            "branch target is {delta} bytes away; relative branches reach -128..+127                              (use LJMP or restructure)"
                        ),
                    );
                    return Vec::new();
                }
                instruction.operands.set(i, Operand::Rel(delta as i8));
                continue;
            }
            let field = match def.enc[i] {
                Enc::Byte(o) | Enc::Word(o) => o as u32,
                Enc::Addr11 => 1,
                _ => {
                    self.error_in(
                        &line.file,
                        line.line,
                        format!("cannot relocate a `{arg:?}` operand"),
                    );
                    return Vec::new();
                }
            };
            instruction.operands.set(i, placeholder(*arg));
            self.sections[section].relocs.push(Reloc {
                offset: offset + field,
                kind,
                symbol,
                addend,
                instr_offset: offset,
                instr_len: def.len(),
            });
        }

        // Range check absolutes that did not become relocations.
        for (i, arg) in def.args.iter().enumerate() {
            let Some(Some(Value::Abs(v))) = values.get(i) else {
                continue;
            };
            let limit: i64 = match arg {
                Arg::Direct | Arg::Bit | Arg::NotBit | Arg::Imm8 => 0xFF,
                _ => 0xFFFF,
            };
            if *v > limit || *v < i64::MIN / 2 {
                self.error_in(
                    &line.file,
                    line.line,
                    format!("value {v:#X} does not fit the {arg:?} operand (max {limit:#X})"),
                );
                return Vec::new();
            }
        }

        match encode(&instruction) {
            Ok(encoded) => encoded.as_slice().to_vec(),
            Err(e) => {
                let message = match e {
                    EncodeError::RelativeOutOfRange { from, to } => format!(
                        "{to:#06X} is out of range of this branch at {from:#06X} \
                         (use LJMP/LCALL or move the target closer)"
                    ),
                    EncodeError::PageOutOfRange { from, to } => format!(
                        "{to:#06X} is not in the same 2 KiB page as {from:#06X} \
                         (use LJMP/LCALL instead)"
                    ),
                    other => other.to_string(),
                };
                self.error_in(&line.file, line.line, message);
                Vec::new()
            }
        }
    }

    fn emit_data(
        &mut self,
        line: &ParsedLine,
        section: usize,
        offset: u32,
        width: u8,
        items: &[DataItem],
    ) -> Vec<u8> {
        let here = self.abs_here(section, offset);
        let mut out = Vec::new();
        for item in items {
            match item {
                DataItem::Text(bytes) => out.extend_from_slice(bytes),
                DataItem::Value(expr) => match self.eval_operand(expr, here) {
                    Ok(Value::Abs(v)) => {
                        if width == 2 {
                            out.push((v >> 8) as u8);
                            out.push(v as u8);
                        } else {
                            out.push(v as u8);
                        }
                    }
                    Ok(Value::Reloc {
                        symbol,
                        addend,
                        transform,
                        ..
                    }) => {
                        let kind = match (width, transform) {
                            (2, Transform::Plain) => RelocKind::Abs16,
                            (1, Transform::Plain) | (1, Transform::Low) => RelocKind::Abs8,
                            (1, Transform::High) => RelocKind::High8,
                            (2, _) => {
                                self.error_in(
                                    &line.file,
                                    line.line,
                                    "low()/high() cannot be stored in a .dw",
                                );
                                continue;
                            }
                            _ => RelocKind::Abs8,
                        };
                        let at = (offset as usize + out.len()) as u32;
                        out.extend(std::iter::repeat_n(0u8, width as usize));
                        self.sections[section].relocs.push(Reloc {
                            offset: at,
                            kind,
                            symbol,
                            addend,
                            instr_offset: at,
                            instr_len: width,
                        });
                    }
                    Err((e, _)) => self.error_in(&line.file, line.line, format!("{e}")),
                },
            }
        }
        out
    }

    fn finish(mut self) -> AssembleOutput {
        let mut object = ObjectFile::new(self.options.filename.clone());
        object.entry = self.entry.clone();

        for section in self.sections.drain(..) {
            if section.size == 0 && section.relocs.is_empty() && !section.org_from_directive {
                continue;
            }
            if section.wants_absolute && section.org.is_none() && section.size > 0 {
                self.diagnostics.push(Diagnostic::error(
                    &self.options.filename,
                    0,
                    format!(
                        "area `{}` is declared absolute but no .org set its address",
                        section.name
                    ),
                ));
            }
            object.sections.push(Section {
                name: section.name,
                space: section.space,
                org: section.org,
                data: section.data,
                relocs: section.relocs,
            });
        }

        for (name, def) in &self.symbols {
            if def.pending.is_some() {
                self.diagnostics.push(Diagnostic::error(
                    &def.file,
                    def.line,
                    format!("`{name}` could not be resolved"),
                ));
                continue;
            }
            if def.file == "<command line>" || def.file == "<builtin>" {
                continue;
            }
            object.symbols.push(Symbol {
                name: name.clone(),
                kind: def.kind,
                loc: def.loc.clone(),
                global: self.globals.contains(name),
                file: def.file.clone(),
                line: def.line,
            });
        }
        object.globals = self.globals.iter().cloned().collect();
        object.refresh_externs();
        for name in &self.externs {
            if !object.externs.contains(name) {
                object.externs.push(name.clone());
            }
        }
        object.externs.sort();

        AssembleOutput {
            object,
            listing: if self.options.listing {
                self.listing
            } else {
                Vec::new()
            },
            diagnostics: self.diagnostics,
        }
    }
}

fn to_form(operand: &OperandText, value: Option<u32>) -> Option<OperandForm> {
    let value = value.unwrap_or(0);
    Some(match operand {
        OperandText::Acc => OperandForm::A,
        OperandText::Ab => OperandForm::Ab,
        OperandText::Carry => OperandForm::C,
        OperandText::Dptr => OperandForm::Dptr,
        OperandText::Rn(n) => OperandForm::Rn(*n),
        OperandText::AtRi(i) => OperandForm::AtRi(*i),
        OperandText::AtDptr => OperandForm::AtDptr,
        OperandText::AtADptr => OperandForm::AtADptr,
        OperandText::AtAPc => OperandForm::AtAPc,
        OperandText::Imm(_) => OperandForm::Imm(value),
        OperandText::NotBit(_) => OperandForm::NotBit(value),
        OperandText::Bare(_) => OperandForm::Bare(value),
        OperandText::Bad(_) => return None,
    })
}

fn reloc_kind(arg: Arg, transform: Transform) -> Result<RelocKind, String> {
    Ok(match (arg, transform) {
        (Arg::Direct | Arg::Bit | Arg::Imm8, Transform::Plain | Transform::Low) => RelocKind::Abs8,
        (Arg::Direct | Arg::Bit | Arg::Imm8, Transform::High) => RelocKind::High8,
        (Arg::Imm16 | Arg::Addr16, Transform::Plain) => RelocKind::Abs16,
        (Arg::Addr11, Transform::Plain) => RelocKind::Addr11,
        (Arg::Rel, Transform::Plain) => RelocKind::Rel8,
        (arg, _) => {
            return Err(format!(
                "a relocatable value cannot be used as the {arg:?} operand of this instruction"
            ));
        }
    })
}

fn placeholder(arg: Arg) -> Operand {
    match arg {
        Arg::Direct => Operand::Direct(0),
        Arg::Bit => Operand::Bit(0),
        Arg::NotBit => Operand::NotBit(0),
        Arg::Imm8 => Operand::Imm8(0),
        Arg::Imm16 => Operand::Imm16(0),
        Arg::Addr11 => Operand::Addr11(0),
        Arg::Addr16 => Operand::Addr16(0),
        Arg::Rel => Operand::Rel(0),
        other => panic!("no placeholder for {other:?}"),
    }
}
