//! # mcs51-ld
//!
//! The 8051 linker: it takes the relocatable objects produced by
//! [`mcs51_asm`], places their sections according to a linker script, resolves
//! symbols, applies relocations, and emits a firmware image plus a map file.
//!
//! This is the `sdld` of the `mcs51-rs` toolchain, with the ASXXXX `.lkr` script
//! replaced by a compact, line-oriented format:
//!
//! ```text
//! ; mcs51-rs linker script
//! memory CODE  0x0000 0x8000
//! memory DATA  0x0030 0x50
//! memory XDATA xdata  0x0000 0x8000
//!
//! section HOME  -> CODE
//! section CODE  -> CODE
//! section BSS   -> DATA
//! section XDATA -> XDATA
//!
//! entry start
//! checksum 0x7FFE
//! ```
//!
//! `memory` takes a name, an optional address space keyword, an origin and a
//! length. `section` maps an input section name onto a memory region and may
//! pin it with `at 0x1234`.

use std::collections::{BTreeMap, BTreeSet};

use mcs51_asm::{ObjectFile, RelocKind, SymbolLoc};
use mcs51_ihex::Image;
use mcs51_isa::mem::Space;

pub mod script;

pub use script::{LinkerScript, Memory, Placement, ScriptError};

/// Where a linked section ended up.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlacedSection {
    /// Output section name.
    pub name: String,
    /// Address space.
    pub space: Space,
    /// Final base address.
    pub addr: u16,
    /// Length in bytes.
    pub len: u32,
    /// Memory region it was placed in.
    pub region: String,
    /// Objects that contributed, with their byte counts.
    pub contributors: Vec<(String, u32)>,
}

/// A symbol after linking.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlacedSymbol {
    /// Symbol name.
    pub name: String,
    /// Final value.
    pub value: u32,
    /// Section it belongs to, if any.
    pub section: Option<String>,
    /// Whether the symbol was exported.
    pub global: bool,
}

/// Severity of a link diagnostic.
pub use mcs51_asm::Severity;

/// A linker message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    /// How serious it is.
    pub severity: Severity,
    /// What went wrong.
    pub message: String,
    /// The object the problem was found in, if any.
    pub object: Option<String>,
}

impl Diagnostic {
    /// An error.
    pub fn error(message: impl Into<String>) -> Self {
        Self {
            severity: Severity::Error,
            message: message.into(),
            object: None,
        }
    }

    /// A warning.
    pub fn warning(message: impl Into<String>) -> Self {
        Self {
            severity: Severity::Warning,
            message: message.into(),
            object: None,
        }
    }

    /// Attach the responsible object.
    pub fn in_object(mut self, object: impl Into<String>) -> Self {
        self.object = Some(object.into());
        self
    }
}

impl std::fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: ", self.severity)?;
        if let Some(object) = &self.object {
            write!(f, "[{object}] ")?;
        }
        f.write_str(&self.message)
    }
}

/// Options for one link.
#[derive(Debug, Clone)]
pub struct LinkOptions {
    /// The linker script.
    pub script: LinkerScript,
    /// Fill byte for gaps inside placed sections.
    pub fill: u8,
    /// Report sections placed outside their memory region as errors.
    pub strict: bool,
}

impl Default for LinkOptions {
    fn default() -> Self {
        Self {
            script: LinkerScript::default_8051(),
            fill: 0xFF,
            strict: true,
        }
    }
}

/// The result of a link.
#[derive(Debug, Clone)]
pub struct LinkOutput {
    /// Program memory image, ready for a ROM programmer.
    pub code: Image,
    /// Initialised data images for the other address spaces.
    pub data: BTreeMap<Space, Image>,
    /// Final symbol table.
    pub symbols: BTreeMap<String, PlacedSymbol>,
    /// Sections in placement order.
    pub sections: Vec<PlacedSection>,
    /// Entry point address, if the script named one.
    pub entry: Option<u32>,
    /// Diagnostics, in discovery order.
    pub diagnostics: Vec<Diagnostic>,
    /// The linker script that was used.
    pub script: LinkerScript,
}

impl LinkOutput {
    /// `true` when the link produced no errors.
    pub fn is_ok(&self) -> bool {
        !self
            .diagnostics
            .iter()
            .any(|d| d.severity == Severity::Error)
    }

    /// Diagnostics rendered one per line.
    pub fn render_diagnostics(&self) -> String {
        self.diagnostics
            .iter()
            .map(|d| d.to_string())
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// A GNU-style map file.
    pub fn map_file(&self, inputs: &[ObjectFile]) -> String {
        let mut out = String::new();
        out.push_str("mcs51-rs link map\n================\n\n");

        out.push_str("Memory regions\n--------------\n");
        for memory in &self.script.memories {
            let used = self
                .sections
                .iter()
                .filter(|s| s.region == memory.name)
                .map(|s| s.len as u64)
                .sum::<u64>();
            let length = memory.length as u64;
            let percent = if length == 0 {
                0.0
            } else {
                used as f64 * 100.0 / length as f64
            };
            out.push_str(&format!(
                "  {:<8} {:<6} {:#06X}..{:#06X}  {:>6} / {:>6} bytes  {:>5.1}% {}\n",
                memory.name,
                memory.space.name(),
                memory.origin,
                memory.origin as u64 + length - 1,
                used,
                length,
                percent,
                if used > length { "OVERFLOW" } else { "" }
            ));
        }

        out.push_str("\nSections\n--------\n");
        for section in &self.sections {
            out.push_str(&format!(
                "  {:<8} {:<6} {:#06X} {:>6} bytes  [{}]\n",
                section.name,
                section.space.name(),
                section.addr,
                section.len,
                section
                    .contributors
                    .iter()
                    .map(|(name, size)| format!("{name}:{size}"))
                    .collect::<Vec<_>>()
                    .join(" ")
            ));
        }

        out.push_str("\nSymbols\n-------\n");
        let mut symbols: Vec<&PlacedSymbol> = self.symbols.values().collect();
        symbols.sort_by_key(|s| (s.value, s.name.clone()));
        for symbol in symbols {
            out.push_str(&format!(
                "  {:#06X} {:<24} {}{}\n",
                symbol.value,
                symbol.name,
                symbol.section.clone().unwrap_or_else(|| "ABS".into()),
                if symbol.global { " (global)" } else { "" }
            ));
        }

        if !inputs.is_empty() {
            out.push_str("\nInput objects\n-------------\n");
            for object in inputs {
                out.push_str(&format!(
                    "  {:<28} {:>6} bytes  {} section(s)\n",
                    object.name,
                    object.total_bytes(),
                    object.sections.len()
                ));
            }
        }
        out
    }

    /// Code size, for a build summary line.
    pub fn code_size(&self) -> usize {
        self.code.len()
    }
}

/// Link a set of objects.
pub fn link(objects: &[ObjectFile], options: &LinkOptions) -> LinkOutput {
    let mut ctx = Context {
        options,
        diagnostics: Vec::new(),
        symbols: BTreeMap::new(),
        sections: Vec::new(),
        images: BTreeMap::new(),
        regions: options
            .script
            .memories
            .iter()
            .map(|m| (m.name.clone(), Cursor::new(m.origin, m.length)))
            .collect(),
        bases: BTreeMap::new(),
    };
    ctx.run(objects);

    let entry = options
        .script
        .entry
        .as_ref()
        .and_then(|name| ctx.symbols.get(name).map(|s| s.value));

    let code = ctx.images.remove(&Space::Code).unwrap_or_default();
    let mut data = ctx.images;
    // Only keep spaces that actually have content.
    data.retain(|_, image| !image.is_empty());

    LinkOutput {
        code,
        data,
        symbols: ctx.symbols,
        sections: ctx.sections,
        entry,
        diagnostics: ctx.diagnostics,
        script: options.script.clone(),
    }
}

struct Cursor {
    origin: u32,
    length: u32,
    next: u32,
}

impl Cursor {
    fn new(origin: u32, length: u32) -> Self {
        Self {
            origin,
            length,
            next: origin,
        }
    }

    fn end(&self) -> u32 {
        self.origin + self.length
    }
}

struct Context<'a> {
    options: &'a LinkOptions,
    diagnostics: Vec<Diagnostic>,
    symbols: BTreeMap<String, PlacedSymbol>,
    sections: Vec<PlacedSection>,
    images: BTreeMap<Space, Image>,
    regions: BTreeMap<String, Cursor>,
    /// Final base address of each `(object, section)` pair.
    bases: BTreeMap<(String, String), u32>,
}

impl<'a> Context<'a> {
    fn run(&mut self, objects: &[ObjectFile]) {
        self.collect_globals(objects);
        self.place(objects);
        self.resolve_symbols(objects);
        self.relocate(objects);
        self.apply_checksum();
    }

    /// Pass 1a: build the global symbol table (addresses are filled in later).
    fn collect_globals(&mut self, objects: &[ObjectFile]) {
        for object in objects {
            for symbol in &object.symbols {
                let existing = self.symbols.get(&symbol.name);
                match existing {
                    Some(previous)
                        if previous.global
                            && symbol.global
                            && previous.section != symbol_section(&symbol.loc) =>
                    {
                        self.diagnostics.push(
                            Diagnostic::error(format!(
                                "symbol `{}` is defined more than once ({} and {})",
                                symbol.name,
                                previous.section.clone().unwrap_or_else(|| "?".into()),
                                symbol_section(&symbol.loc).unwrap_or_else(|| "?".into())
                            ))
                            .in_object(object.name.clone()),
                        );
                    }
                    Some(previous) if !previous.global && previous.value == 0 => {}
                    _ => {}
                }
                // The first definition wins; later local duplicates are
                // per-object labels and were already uniquified by the
                // assembler's section scoping.
                self.symbols
                    .entry(symbol.name.clone())
                    .or_insert_with(|| PlacedSymbol {
                        name: symbol.name.clone(),
                        value: 0,
                        section: symbol_section(&symbol.loc),
                        global: symbol.global,
                    });
            }
        }
    }

    /// Pass 1b: place every input section and copy its bytes into the images.
    fn place(&mut self, objects: &[ObjectFile]) {
        // Walk the script's section order so that placement is deterministic and
        // `HOME` can be pinned ahead of the rest of `CODE`, even when several
        // objects contribute to the same output section.
        let mut order: Vec<String> = Vec::new();
        for placement in &self.options.script.placements {
            if !order.contains(&placement.section) {
                order.push(placement.section.clone());
            }
        }
        for object in objects {
            for section in &object.sections {
                if !order.contains(&section.name) {
                    order.push(section.name.clone());
                }
            }
        }

        for name in order {
            let mut contributors = Vec::new();
            let mut base: Option<u32> = None;
            let mut space = Space::Code;
            let mut region_name = String::from("CODE");
            let mut total = 0u32;

            for object in objects {
                for section in object.sections.iter().filter(|s| s.name == name) {
                    space = section.space;
                    let placement = self.options.script.placement_for(&name);
                    let Some(placement) = placement else {
                        self.diagnostics.push(
                            Diagnostic::error(format!(
                                "section `{name}` is not mentioned in the linker script"
                            ))
                            .in_object(object.name.clone()),
                        );
                        continue;
                    };
                    region_name = placement.region.clone();
                    let Some(cursor) = self.regions.get_mut(&placement.region) else {
                        self.diagnostics.push(Diagnostic::error(format!(
                            "section `{name}` names unknown memory region `{}`",
                            placement.region
                        )));
                        continue;
                    };

                    let wanted = match (placement.at, section.org) {
                        (Some(at), _) => Some(at as u32),
                        (None, Some(org)) => Some(org as u32),
                        (None, None) => None,
                    };
                    if let Some(wanted) = wanted {
                        if wanted < cursor.origin
                            || wanted + section.data.len() as u32 > cursor.end()
                        {
                            self.diagnostics.push(
                                Diagnostic::error(format!(
                                    "section `{name}` at {wanted:#06X}..{:#06X} does not fit \
                                     region `{}` ({:#06X}..{:#06X})",
                                    wanted as usize + section.data.len(),
                                    placement.region,
                                    cursor.origin,
                                    cursor.end()
                                ))
                                .in_object(object.name.clone()),
                            );
                        }
                        if wanted > cursor.next {
                            cursor.next = wanted;
                        }
                        if wanted < cursor.next {
                            self.diagnostics.push(
                                Diagnostic::error(format!(
                                    "section `{name}` wants {wanted:#06X} but region `{}` has \
                                     already reached {:#06X} (overlap)",
                                    placement.region, cursor.next
                                ))
                                .in_object(object.name.clone()),
                            );
                        }
                    }

                    let addr = cursor.next;
                    let len = section.data.len() as u32;
                    if addr + len > cursor.end() {
                        self.diagnostics.push(
                            Diagnostic::error(format!(
                                "section `{name}` overflows region `{}`: needs {len} bytes at \
                                 {addr:#06X}, region ends at {:#06X}",
                                placement.region,
                                cursor.end()
                            ))
                            .in_object(object.name.clone()),
                        );
                    }
                    cursor.next = addr + len.max(1);

                    if base.is_none() {
                        base = Some(addr);
                    }
                    // Record this object's contribution so the map file can show
                    // where each section's bytes came from.
                    self.patch_section(object, section, addr, &mut contributors);

                    // Remember the per-object/per-section base so relocation can
                    // look it up.
                    self.bases
                        .insert((object.name.clone(), section.name.clone()), addr);
                    total += len;
                }
            }

            if let Some(base) = base {
                self.sections.push(PlacedSection {
                    name: name.clone(),
                    space,
                    addr: base as u16,
                    len: total,
                    region: region_name,
                    contributors,
                });
            }
        }
    }

    fn patch_section(
        &mut self,
        object: &ObjectFile,
        section: &mcs51_asm::Section,
        addr: u32,
        contributors: &mut Vec<(String, u32)>,
    ) {
        if section.data.is_empty() {
            return;
        }
        if contributors.iter().all(|(name, _)| name != &object.name) {
            contributors.push((object.name.clone(), 0));
        }
        if let Some(entry) = contributors.iter_mut().find(|(n, _)| n == &object.name) {
            entry.1 += section.data.len() as u32;
        }
        let image = self.images.entry(section.space).or_default();
        if let Err(error) = image.write_checked(addr as u16, &section.data) {
            self.diagnostics
                .push(Diagnostic::error(error.to_string()).in_object(object.name.clone()));
        }
    }

    /// Pass 2: turn every symbol definition into a final value.
    fn resolve_symbols(&mut self, objects: &[ObjectFile]) {
        for object in objects {
            for symbol in &object.symbols {
                let value = match &symbol.loc {
                    SymbolLoc::Absolute(v) => *v,
                    SymbolLoc::Section { section, offset } => {
                        match self.bases.get(&(object.name.clone(), section.clone())) {
                            Some(base) => *base as i64 + *offset as i64,
                            None => {
                                self.diagnostics.push(
                                    Diagnostic::error(format!(
                                        "symbol `{}` refers to missing section `{section}`",
                                        symbol.name
                                    ))
                                    .in_object(object.name.clone()),
                                );
                                continue;
                            }
                        }
                    }
                };
                let entry =
                    self.symbols
                        .entry(symbol.name.clone())
                        .or_insert_with(|| PlacedSymbol {
                            name: symbol.name.clone(),
                            value: 0,
                            section: symbol_section(&symbol.loc),
                            global: symbol.global,
                        });
                // Global definitions override tentative entries; locals only
                // fill in an empty slot.
                if symbol.global || entry.value == 0 {
                    entry.value = value as u32;
                    entry.global |= symbol.global;
                    entry.section = symbol_section(&symbol.loc);
                }
            }
        }
    }

    /// Pass 3: apply every relocation.
    fn relocate(&mut self, objects: &[ObjectFile]) {
        for object in objects {
            for section in &object.sections {
                let Some(base) = self.bases.get(&(object.name.clone(), section.name.clone()))
                else {
                    continue;
                };
                let base = *base;
                let mut patches: Vec<(u32, Vec<u8>)> = Vec::new();
                for reloc in &section.relocs {
                    let Some(target) = self.symbols.get(&reloc.symbol) else {
                        self.diagnostics.push(
                            Diagnostic::error(format!(
                                "undefined symbol `{}` referenced from {} offset {:#06X}",
                                reloc.symbol, section.name, reloc.offset
                            ))
                            .in_object(object.name.clone()),
                        );
                        continue;
                    };
                    let value = target.value as i64 + reloc.addend;
                    let mut emit = |bytes: Vec<u8>| patches.push((reloc.offset, bytes));
                    match reloc.kind {
                        RelocKind::Abs8 => emit(vec![value as u8]),
                        RelocKind::High8 => emit(vec![((value >> 8) & 0xFF) as u8]),
                        RelocKind::Abs16 => {
                            emit(vec![((value >> 8) & 0xFF) as u8, (value & 0xFF) as u8])
                        }
                        RelocKind::Rel8 => {
                            let next =
                                base as i64 + reloc.instr_offset as i64 + reloc.instr_len as i64;
                            let delta = value - next;
                            if !(-128..=127).contains(&delta) {
                                self.diagnostics.push(
                                    Diagnostic::error(format!(
                                        "relative branch to `{}` from {} offset {:#06X} is {delta} \
                                         bytes away; relative branches reach -128..+127",
                                        reloc.symbol, section.name, reloc.instr_offset
                                    ))
                                    .in_object(object.name.clone()),
                                );
                                continue;
                            }
                            emit(vec![delta as i8 as u8]);
                        }
                        RelocKind::Addr11 => {
                            let next =
                                base as i64 + reloc.instr_offset as i64 + reloc.instr_len as i64;
                            if (value as u32 & 0xF800) != (next as u32 & 0xF800) {
                                self.diagnostics.push(
                                    Diagnostic::error(format!(
                                        "AJMP/ACALL to `{}` from {} offset {:#06X} is outside its \
                                         2 KiB page",
                                        reloc.symbol, section.name, reloc.instr_offset
                                    ))
                                    .in_object(object.name.clone()),
                                );
                                continue;
                            }
                            let raw = value as u32 & 0x07FF;
                            // Byte at the relocation point; opcode page bits are
                            // patched below once we know the opcode byte.
                            emit(vec![raw as u8]);
                            patches.push((
                                reloc.offset - 1,
                                vec![
                                    section.data[(reloc.offset - 1) as usize]
                                        | (((raw >> 8) as u8) & 0x07) << 5,
                                ],
                            ));
                        }
                    }
                }

                let image = self.images.entry(section.space).or_default();
                for (offset, bytes) in patches {
                    let at = base as u16 + offset as u16;
                    for (i, byte) in bytes.iter().enumerate() {
                        image.set(at.wrapping_add(i as u16), *byte);
                    }
                }
            }
        }
    }

    fn apply_checksum(&mut self) {
        let Some((at, space)) = self.options.script.checksum else {
            return;
        };
        let image = self.images.entry(space).or_default();
        if image.is_empty() {
            self.diagnostics.push(Diagnostic::warning(format!(
                "checksum requested at {at:#06X} but the {} image is empty",
                space.name()
            )));
            return;
        }
        let sum = image
            .iter()
            .filter(|(addr, _)| *addr != at)
            .fold(0u8, |acc, (_, byte)| acc.wrapping_add(byte));
        image.set(at, sum.wrapping_neg());
    }
}

fn symbol_section(loc: &SymbolLoc) -> Option<String> {
    match loc {
        SymbolLoc::Section { section, .. } => Some(section.clone()),
        SymbolLoc::Absolute(_) => None,
    }
}

/// Count the bytes each section contributes, for a size report.
pub fn section_sizes(objects: &[ObjectFile]) -> BTreeMap<String, usize> {
    let mut sizes: BTreeMap<String, usize> = BTreeMap::new();
    for object in objects {
        for section in &object.sections {
            *sizes.entry(section.name.clone()).or_default() += section.data.len();
        }
    }
    sizes
}

/// Every symbol name an object references but does not define.
pub fn unresolved(objects: &[ObjectFile]) -> BTreeSet<String> {
    let mut needed = BTreeSet::new();
    for object in objects {
        for section in &object.sections {
            for reloc in &section.relocs {
                needed.insert(reloc.symbol.clone());
            }
        }
    }
    for object in objects {
        for symbol in &object.symbols {
            needed.remove(&symbol.name);
        }
    }
    needed
}
