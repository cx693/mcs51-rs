//! Linker scripts.

use mcs51_isa::mem::Space;

/// A named address range.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Memory {
    /// Region name, e.g. `CODE`.
    pub name: String,
    /// Address space the region belongs to.
    pub space: Space,
    /// First address.
    pub origin: u32,
    /// Number of bytes available.
    pub length: u32,
}

/// A rule mapping an input section onto a memory region.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Placement {
    /// Input section name.
    pub section: String,
    /// Region it is placed in.
    pub region: String,
    /// Fixed address, overriding the region cursor.
    pub at: Option<u32>,
}

/// A parsed linker script.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkerScript {
    /// Declared memory regions, in order.
    pub memories: Vec<Memory>,
    /// Section placement rules, in order.
    pub placements: Vec<Placement>,
    /// Entry point symbol, if `entry` was given.
    pub entry: Option<String>,
    /// Optional `(address, space)` for a firmware checksum byte.
    pub checksum: Option<(u16, Space)>,
}

impl Default for LinkerScript {
    fn default() -> Self {
        Self::default_8051()
    }
}

impl LinkerScript {
    /// A script describing a stock 8051 with 32 KiB of program memory, the
    /// upper half of internal RAM and 32 KiB of external data.
    ///
    /// This is what a project gets when it does not ship a `.lk` file, so that a
    /// single-file build works with no configuration at all. The `HOME` section
    /// and the ordinary `CODE` section share one region: the assembler pins the
    /// interrupt vectors with `.org`, and everything after them follows on.
    pub fn default_8051() -> Self {
        Self {
            memories: vec![
                Memory {
                    name: "CODE".into(),
                    space: Space::Code,
                    origin: 0x0000,
                    length: 0x8000,
                },
                Memory {
                    name: "DATA".into(),
                    space: Space::Data,
                    origin: 0x0030,
                    length: 0x50,
                },
                Memory {
                    name: "IDATA".into(),
                    space: Space::Idata,
                    origin: 0x0030,
                    length: 0xD0,
                },
                Memory {
                    name: "XDATA".into(),
                    space: Space::Xdata,
                    origin: 0x0000,
                    length: 0x8000,
                },
                Memory {
                    name: "BIT".into(),
                    space: Space::Bit,
                    origin: 0x0000,
                    length: 0x80,
                },
            ],
            placements: vec![
                Placement {
                    section: "HOME".into(),
                    region: "CODE".into(),
                    at: None,
                },
                Placement {
                    section: "CODE".into(),
                    region: "CODE".into(),
                    at: None,
                },
                Placement {
                    section: "DATA".into(),
                    region: "DATA".into(),
                    at: None,
                },
                Placement {
                    section: "BSS".into(),
                    region: "DATA".into(),
                    at: None,
                },
                Placement {
                    section: "IDATA".into(),
                    region: "IDATA".into(),
                    at: None,
                },
                Placement {
                    section: "XDATA".into(),
                    region: "XDATA".into(),
                    at: None,
                },
                Placement {
                    section: "BIT".into(),
                    region: "BIT".into(),
                    at: None,
                },
            ],
            entry: None,
            checksum: None,
        }
    }

    /// Find the placement rule for a section.
    pub fn placement_for(&self, section: &str) -> Option<&Placement> {
        self.placements.iter().find(|p| p.section == section)
    }

    /// Find a memory region by name.
    pub fn memory(&self, name: &str) -> Option<&Memory> {
        self.memories.iter().find(|m| m.name == name)
    }

    /// Parse a linker script.
    pub fn parse(text: &str) -> Result<Self, ScriptError> {
        let mut script = Self {
            memories: Vec::new(),
            placements: Vec::new(),
            entry: None,
            checksum: None,
        };
        for (index, raw) in text.lines().enumerate() {
            let line_no = index + 1;
            let line = strip_comment(raw).trim();
            if line.is_empty() {
                continue;
            }
            let fields: Vec<&str> = line.split_whitespace().collect();
            match fields[0].to_ascii_lowercase().as_str() {
                "memory" | "mem" => {
                    if fields.len() < 4 {
                        return Err(ScriptError::new(
                            line_no,
                            "memory takes: NAME [SPACE] ORIGIN LENGTH",
                        ));
                    }
                    let name = fields[1].to_string();
                    let (space, rest) = match parse_space(fields[2]) {
                        Some(space) => (space, &fields[3..]),
                        None => (space_from_name(&name), &fields[2..]),
                    };
                    if rest.len() < 2 {
                        return Err(ScriptError::new(
                            line_no,
                            "memory needs an origin and a length",
                        ));
                    }
                    let origin = parse_number(rest[0])
                        .ok_or_else(|| ScriptError::new(line_no, "bad origin"))?;
                    let length = parse_number(rest[1])
                        .ok_or_else(|| ScriptError::new(line_no, "bad length"))?;
                    if origin > 0xFFFF {
                        return Err(ScriptError::new(line_no, "origin is above 0xFFFF"));
                    }
                    script.memories.push(Memory {
                        name,
                        space,
                        origin,
                        length,
                    });
                }
                "section" | "seg" => {
                    // section NAME [at ADDR] -> REGION
                    if fields.len() < 4 {
                        return Err(ScriptError::new(
                            line_no,
                            "section takes: NAME -> REGION [at ADDRESS]",
                        ));
                    }
                    let name = fields[1].to_string();
                    let arrow = fields
                        .iter()
                        .position(|f| *f == "->")
                        .ok_or_else(|| ScriptError::new(line_no, "section needs `-> REGION`"))?;
                    let region = fields
                        .get(arrow + 1)
                        .ok_or_else(|| ScriptError::new(line_no, "section needs a region name"))?
                        .to_string();
                    let at = if let Some(pos) = fields.iter().position(|f| *f == "at") {
                        Some(
                            parse_number(
                                fields.get(pos + 1).ok_or_else(|| {
                                    ScriptError::new(line_no, "`at` needs a value")
                                })?,
                            )
                            .ok_or_else(|| ScriptError::new(line_no, "bad address after `at`"))?,
                        )
                    } else {
                        None
                    };
                    script.placements.push(Placement {
                        section: name,
                        region,
                        at: at.map(|v| v as u32),
                    });
                }
                "entry" => {
                    let name = fields
                        .get(1)
                        .ok_or_else(|| ScriptError::new(line_no, "entry needs a symbol"))?;
                    script.entry = Some(name.to_string());
                }
                "checksum" => {
                    let value =
                        parse_number(fields.get(1).ok_or_else(|| {
                            ScriptError::new(line_no, "checksum needs an address")
                        })?)
                        .ok_or_else(|| ScriptError::new(line_no, "bad checksum address"))?;
                    let space = fields
                        .get(2)
                        .and_then(|f| parse_space(f))
                        .unwrap_or(Space::Code);
                    script.checksum = Some((value as u16, space));
                }
                "fill" => { /* accepted and ignored: the CLI sets the fill byte */ }
                other => {
                    return Err(ScriptError::new(
                        line_no,
                        format!("unknown linker script directive `{other}`"),
                    ));
                }
            }
        }
        if script.memories.is_empty() {
            return Err(ScriptError::new(
                0,
                "the linker script declares no memory regions",
            ));
        }
        Ok(script)
    }
}

/// Why a linker script could not be parsed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScriptError {
    /// 1-based line number, or 0 for whole-file problems.
    pub line: usize,
    /// What went wrong.
    pub message: String,
}

impl ScriptError {
    fn new(line: usize, message: impl Into<String>) -> Self {
        Self {
            line,
            message: message.into(),
        }
    }
}

impl std::fmt::Display for ScriptError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.line == 0 {
            write!(f, "{}", self.message)
        } else {
            write!(f, "line {}: {}", self.line, self.message)
        }
    }
}

impl std::error::Error for ScriptError {}

fn strip_comment(line: &str) -> &str {
    match line.find(';') {
        Some(i) => &line[..i],
        None => line,
    }
}

fn parse_space(text: &str) -> Option<Space> {
    Some(match text.to_ascii_lowercase().as_str() {
        "code" | "rom" | "flash" => Space::Code,
        "data" | "ram" | "iram" => Space::Data,
        "idata" => Space::Idata,
        "xdata" | "xram" => Space::Xdata,
        "bit" | "bseg" => Space::Bit,
        "sfr" => Space::Sfr,
        _ => return None,
    })
}

fn space_from_name(name: &str) -> Space {
    parse_space(name).unwrap_or(Space::Code)
}

fn parse_number(text: &str) -> Option<u32> {
    let text = text.trim();
    if let Some(hex) = text.strip_prefix("0x").or_else(|| text.strip_prefix("0X")) {
        return u32::from_str_radix(hex, 16).ok();
    }
    text.parse::<u32>().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_script_covers_the_standard_sections() {
        let script = LinkerScript::default_8051();
        assert!(script.placement_for("HOME").is_some());
        assert!(script.placement_for("CODE").is_some());
        assert_eq!(script.memory("CODE").unwrap().origin, 0x0000);
    }

    #[test]
    fn parses_the_line_format() {
        let script = LinkerScript::parse(
            "\
; comment
memory CODE 0x0000 0x8000
memory XDATA xdata 0x0000 0x1000
section HOME -> CODE
section CODE -> CODE at 0x0100
entry main
checksum 0x7FFE code
",
        )
        .unwrap();
        assert_eq!(script.memories.len(), 2);
        assert_eq!(script.memories[1].space, Space::Xdata);
        assert_eq!(script.placement_for("CODE").unwrap().at, Some(0x0100));
        assert_eq!(script.entry.as_deref(), Some("main"));
        assert_eq!(script.checksum, Some((0x7FFE, Space::Code)));
    }

    #[test]
    fn rejects_broken_scripts() {
        assert!(LinkerScript::parse("memory CODE\n").is_err());
        assert!(LinkerScript::parse("section A B\n").is_err());
        assert!(LinkerScript::parse("nonsense 1 2 3\n").is_err());
        assert!(LinkerScript::parse("; nothing\n").is_err());
    }
}
