//! The relocatable object format shared by the assembler and the linker.

use std::collections::BTreeMap;

use mcs51_isa::mem::Space;

/// A named, contiguous region of one address space.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Section {
    /// Section name, e.g. `CODE` or `DATA`.
    pub name: String,
    /// Which 8051 address space it lives in.
    pub space: Space,
    /// Base address, when the section is absolute.
    ///
    /// `None` means "wherever the linker puts it".
    pub org: Option<u16>,
    /// Encoded bytes.
    pub data: Vec<u8>,
    /// Fixups into the symbol table.
    pub relocs: Vec<Reloc>,
}

impl Section {
    /// A new empty section.
    pub fn new(name: impl Into<String>, space: Space, org: Option<u16>) -> Self {
        Self {
            name: name.into(),
            space,
            org,
            data: Vec::new(),
            relocs: Vec::new(),
        }
    }

    /// `true` when the section has a fixed base address.
    pub fn is_absolute(&self) -> bool {
        self.org.is_some()
    }

    /// Number of bytes in the section.
    pub fn len(&self) -> usize {
        self.data.len()
    }

    /// `true` when the section holds no bytes.
    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }

    /// The address just past the last byte, when the section is absolute.
    pub fn end(&self) -> Option<u32> {
        self.org.map(|o| o as u32 + self.data.len() as u32)
    }

    /// Address of byte `offset` in this section, when absolute.
    pub fn addr_of(&self, offset: u32) -> Option<u32> {
        self.org.map(|o| o as u32 + offset)
    }
}

/// How a relocation computes its value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RelocKind {
    /// One byte: the low 8 bits of the symbol address.
    Abs8,
    /// One byte: bits 8..15 of the symbol address.
    High8,
    /// Two bytes, big-endian: the full symbol address.
    Abs16,
    /// One byte plus opcode page bits: `AJMP`/`ACALL` within the current 2 KiB page.
    Addr11,
    /// One signed byte: the displacement from the end of the instruction.
    Rel8,
}

impl RelocKind {
    /// How many bytes the fixup patches.
    pub const fn width(self) -> u8 {
        match self {
            RelocKind::Abs8 | RelocKind::High8 | RelocKind::Addr11 | RelocKind::Rel8 => 1,
            RelocKind::Abs16 => 2,
        }
    }

    /// Short name, as printed in map files.
    pub const fn as_str(self) -> &'static str {
        match self {
            RelocKind::Abs8 => "ABS8",
            RelocKind::High8 => "HIGH8",
            RelocKind::Abs16 => "ABS16",
            RelocKind::Addr11 => "ADDR11",
            RelocKind::Rel8 => "REL8",
        }
    }
}

impl std::fmt::Display for RelocKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A pending fixup inside a section.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reloc {
    /// Byte offset within [`Section::data`] that gets patched.
    pub offset: u32,
    /// How to compute the value.
    pub kind: RelocKind,
    /// Symbol the relocation refers to.
    pub symbol: String,
    /// Constant added to the symbol's address.
    pub addend: i64,
    /// Section offset of the *instruction* the fixup belongs to.
    ///
    /// `Rel8` and `Addr11` are relative to the end of the instruction, so the
    /// linker needs to know where the instruction starts, not just where the
    /// operand byte is.
    pub instr_offset: u32,
    /// Length in bytes of the instruction, for `Rel8`/`Addr11`.
    pub instr_len: u8,
}

impl Reloc {
    /// A relocation whose instruction begins at the patched operand.
    pub fn new(offset: u32, kind: RelocKind, symbol: impl Into<String>) -> Self {
        Self {
            offset,
            kind,
            symbol: symbol.into(),
            addend: 0,
            instr_offset: offset,
            instr_len: kind.width(),
        }
    }

    /// Set the addend.
    pub fn with_addend(mut self, addend: i64) -> Self {
        self.addend = addend;
        self
    }
}

impl std::fmt::Display for Reloc {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "+{:#06X} {} {}{:+}",
            self.offset, self.kind, self.symbol, self.addend
        )
    }
}

/// What a symbol denotes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SymbolKind {
    /// A label: the address of a location in a section.
    Label,
    /// An absolute constant from `.equ`/`.set`.
    Equate,
    /// The address of an SFR, declared with `.sfr`.
    Sfr,
    /// A bit address, declared with `.bit`.
    Bit,
}

impl SymbolKind {
    /// Short name for map files.
    pub const fn as_str(self) -> &'static str {
        match self {
            SymbolKind::Label => "label",
            SymbolKind::Equate => "equ",
            SymbolKind::Sfr => "sfr",
            SymbolKind::Bit => "bit",
        }
    }
}

/// Where a symbol lives.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SymbolLoc {
    /// A plain number.
    Absolute(i64),
    /// An offset from the start of a section.
    Section {
        /// Section name.
        section: String,
        /// Byte offset within that section.
        offset: u32,
    },
}

/// A symbol definition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Symbol {
    /// Symbol name.
    pub name: String,
    /// What the symbol denotes.
    pub kind: SymbolKind,
    /// Where it lives.
    pub loc: SymbolLoc,
    /// Whether other objects may reference it.
    pub global: bool,
    /// The defining source file, for diagnostics.
    pub file: String,
    /// The defining source line, for diagnostics.
    pub line: usize,
}

impl Symbol {
    /// A new label in a section.
    pub fn label(
        name: impl Into<String>,
        section: impl Into<String>,
        offset: u32,
        file: impl Into<String>,
        line: usize,
    ) -> Self {
        Self {
            name: name.into(),
            kind: SymbolKind::Label,
            loc: SymbolLoc::Section {
                section: section.into(),
                offset,
            },
            global: false,
            file: file.into(),
            line,
        }
    }

    /// A new absolute symbol.
    pub fn equate(
        name: impl Into<String>,
        value: i64,
        kind: SymbolKind,
        file: impl Into<String>,
        line: usize,
    ) -> Self {
        Self {
            name: name.into(),
            kind,
            loc: SymbolLoc::Absolute(value),
            global: false,
            file: file.into(),
            line,
        }
    }

    /// `true` when the symbol resolves without linking.
    pub fn is_absolute(&self) -> bool {
        matches!(self.loc, SymbolLoc::Absolute(_))
    }
}

impl std::fmt::Display for Symbol {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.loc {
            SymbolLoc::Absolute(v) => write!(f, "{} = {v:#X} ({})", self.name, self.kind.as_str()),
            SymbolLoc::Section { section, offset } => write!(
                f,
                "{} = {}+{offset:#X} ({})",
                self.name,
                section,
                self.kind.as_str()
            ),
        }
    }
}

/// A relocatable object file: the unit of assembly output.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ObjectFile {
    /// Where this object came from.
    pub name: String,
    /// Sections in declaration order.
    pub sections: Vec<Section>,
    /// Defined symbols.
    pub symbols: Vec<Symbol>,
    /// Symbols referenced but not defined here.
    pub externs: Vec<String>,
    /// Symbols exported to other objects.
    pub globals: Vec<String>,
    /// Entry point symbol named by `.entry`, if any.
    pub entry: Option<String>,
}

impl ObjectFile {
    /// An empty object.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            ..Self::default()
        }
    }

    /// Find a section by name.
    pub fn section(&self, name: &str) -> Option<&Section> {
        self.sections.iter().find(|s| s.name == name)
    }

    /// Find a section by name, mutably.
    pub fn section_mut(&mut self, name: &str) -> Option<&mut Section> {
        self.sections.iter_mut().find(|s| s.name == name)
    }

    /// Get or create a section.
    pub fn section_or_insert(
        &mut self,
        name: &str,
        space: Space,
        org: Option<u16>,
    ) -> &mut Section {
        if let Some(idx) = self.sections.iter().position(|s| s.name == name) {
            return &mut self.sections[idx];
        }
        self.sections.push(Section::new(name, space, org));
        self.sections.last_mut().expect("just pushed")
    }

    /// Find a symbol by name.
    pub fn symbol(&self, name: &str) -> Option<&Symbol> {
        self.symbols.iter().find(|s| s.name == name)
    }

    /// Symbols by name.
    pub fn symbol_map(&self) -> BTreeMap<&str, &Symbol> {
        self.symbols.iter().map(|s| (s.name.as_str(), s)).collect()
    }

    /// Total bytes of section data.
    pub fn total_bytes(&self) -> usize {
        self.sections.iter().map(|s| s.data.len()).sum()
    }

    /// Recompute the extern list from the relocations and definitions.
    pub fn refresh_externs(&mut self) {
        let defined: std::collections::BTreeSet<&str> =
            self.symbols.iter().map(|s| s.name.as_str()).collect();
        let mut externs: Vec<String> = Vec::new();
        for section in &self.sections {
            for reloc in &section.relocs {
                if !defined.contains(reloc.symbol.as_str()) && !externs.contains(&reloc.symbol) {
                    externs.push(reloc.symbol.clone());
                }
            }
        }
        externs.sort();
        self.externs = externs;
    }

    /// A human-readable summary: sections and their sizes.
    pub fn summary(&self) -> String {
        let mut out = format!("object {}\n", self.name);
        for section in &self.sections {
            out.push_str(&format!(
                "  {:<8} {:<5} {:#06X?}  {:>5} bytes  {} reloc(s)\n",
                section.name,
                section.space.name(),
                section.org.unwrap_or(0),
                section.data.len(),
                section.relocs.len()
            ));
        }
        for symbol in &self.symbols {
            out.push_str(&format!("  {symbol}\n"));
        }
        out
    }
}
