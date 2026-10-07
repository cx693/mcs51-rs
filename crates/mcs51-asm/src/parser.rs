//! Line and operand parsing: assembly text into [`Statement`]s.

use std::path::{Path, PathBuf};

use mcs51_isa::{Mnemonic, mem::Space};

use crate::expr::{self, Expr};
use crate::object::SymbolKind;
use crate::{Diagnostic, Options, builtin_include};

/// One operand, as written in the source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OperandText {
    /// `A`
    Acc,
    /// `AB`
    Ab,
    /// `C`
    Carry,
    /// `DPTR`
    Dptr,
    /// `R0`..`R7`
    Rn(u8),
    /// `@R0` / `@R1`
    AtRi(u8),
    /// `@DPTR`
    AtDptr,
    /// `@A+DPTR`
    AtADptr,
    /// `@A+PC`
    AtAPc,
    /// `#expr`
    Imm(Expr),
    /// `/expr`
    NotBit(Expr),
    /// A bare expression.
    Bare(Expr),
    /// Text that could not be understood; kept for the error message.
    Bad(String),
}

/// One item of a `.db`/`.dw` list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DataItem {
    /// A value.
    Value(Expr),
    /// A literal string, expanded to bytes.
    Text(Vec<u8>),
}

/// A parsed statement.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Statement {
    /// A blank or comment-only line.
    Empty,
    /// `MNEMONIC operands...`
    Instruction {
        /// The mnemonic.
        mnemonic: Mnemonic,
        /// Its operands.
        operands: Vec<OperandText>,
    },
    /// `.db` / `.dw`
    Data {
        /// Element width in bytes: 1 or 2.
        width: u8,
        /// The elements.
        items: Vec<DataItem>,
    },
    /// `.ds count [, fill]`
    Reserve {
        /// Bytes to reserve.
        count: Expr,
        /// Fill byte.
        fill: Option<Expr>,
    },
    /// `.org expr`
    Org(Expr),
    /// `.align expr`
    Align(Expr),
    /// `.area NAME (ABS)`
    Area {
        /// Area name.
        name: String,
        /// Address space the area lives in.
        space: Space,
        /// Whether the area has a fixed address.
        absolute: bool,
    },
    /// `.equ` / `.set` / `.bit` / `.sfr`
    Define {
        /// Symbol being defined.
        name: String,
        /// Its value.
        expr: Expr,
        /// What kind of symbol it is.
        kind: SymbolKind,
    },
    /// `.globl`
    Globals(Vec<String>),
    /// `.extern`
    Externs(Vec<String>),
    /// `.entry`
    Entry(String),
    /// `.end`
    End,
    /// An unrecognised directive.
    Unknown {
        /// The directive text.
        directive: String,
        /// Its arguments.
        args: String,
    },
}

/// One parsed line of assembly.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedLine {
    /// Source file.
    pub file: String,
    /// 1-based line number in that file.
    pub line: usize,
    /// The original text.
    pub text: String,
    /// Label defined on this line, if any.
    pub label: Option<String>,
    /// `true` when the label starts with `.` and is scoped to the previous
    /// global label.
    pub label_is_local: bool,
    /// What the line does.
    pub statement: Statement,
}

/// Strip a trailing comment, respecting string literals.
pub fn strip_comment(line: &str) -> &str {
    let bytes = line.as_bytes();
    let mut quote: Option<u8> = None;
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i];
        match quote {
            Some(q) => {
                if c == b'\\' {
                    i += 1;
                } else if c == q {
                    quote = None;
                }
            }
            None => {
                if c == b'"' || c == b'\'' {
                    quote = Some(c);
                } else if c == b';' || (c == b'/' && i + 1 < bytes.len() && bytes[i + 1] == b'/') {
                    // 8051 汇编的注释符是 `;`；`//` 也接受，因为生成的代码里
                    // 混着两种风格会让人抓狂。
                    return &line[..i];
                }
            }
        }
        i += 1;
    }
    line
}

/// Split on commas at the top level, respecting parentheses and strings.
pub fn split_operands(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut depth = 0i32;
    let mut quote: Option<char> = None;
    for c in text.chars() {
        match quote {
            Some(q) => {
                current.push(c);
                if c == q {
                    quote = None;
                }
            }
            None => match c {
                '"' | '\'' => {
                    quote = Some(c);
                    current.push(c);
                }
                '(' | '[' => {
                    depth += 1;
                    current.push(c);
                }
                ')' | ']' => {
                    depth -= 1;
                    current.push(c);
                }
                ',' if depth == 0 => {
                    out.push(current.trim().to_string());
                    current.clear();
                }
                _ => current.push(c),
            },
        }
    }
    if !current.trim().is_empty() || !out.is_empty() {
        out.push(current.trim().to_string());
    }
    out
}

/// Parse an operand.
pub fn parse_operand(text: &str) -> OperandText {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return OperandText::Bad(String::new());
    }

    if let Some(rest) = trimmed.strip_prefix('#') {
        return match expr::parse(rest) {
            Some(e) => OperandText::Imm(e),
            None => OperandText::Bad(trimmed.to_string()),
        };
    }
    if let Some(rest) = trimmed.strip_prefix('/') {
        return match expr::parse(rest) {
            Some(e) => OperandText::NotBit(e),
            None => OperandText::Bad(trimmed.to_string()),
        };
    }
    if let Some(rest) = trimmed.strip_prefix('@') {
        let up: String = rest.chars().filter(|c| !c.is_whitespace()).collect();
        let up = up.to_ascii_uppercase();
        return match up.as_str() {
            "DPTR" => OperandText::AtDptr,
            "A+DPTR" => OperandText::AtADptr,
            "A+PC" => OperandText::AtAPc,
            "R0" => OperandText::AtRi(0),
            "R1" => OperandText::AtRi(1),
            _ => OperandText::Bad(trimmed.to_string()),
        };
    }

    let upper = trimmed.to_ascii_uppercase();
    match upper.as_str() {
        "A" => return OperandText::Acc,
        "AB" => return OperandText::Ab,
        "C" => return OperandText::Carry,
        "DPTR" => return OperandText::Dptr,
        _ => {}
    }
    if upper.len() == 2
        && upper.starts_with('R')
        && let Some(n) = upper.as_bytes()[1].checked_sub(b'0')
        && n < 8
    {
        return OperandText::Rn(n);
    }

    match expr::parse(trimmed) {
        Some(e) => OperandText::Bare(e),
        None => OperandText::Bad(trimmed.to_string()),
    }
}

fn space_for_area(name: &str) -> Space {
    match name.to_ascii_uppercase().as_str() {
        "DATA" | "DSEG" | "IDATA" | "ISEG" | "BSS" | "BSEG" | "BIT" => {
            match name.to_ascii_uppercase().as_str() {
                "IDATA" | "ISEG" => Space::Idata,
                "BSEG" | "BIT" => Space::Bit,
                _ => Space::Data,
            }
        }
        "XDATA" | "XSEG" => Space::Xdata,
        "SFR" => Space::Sfr,
        _ => Space::Code,
    }
}

/// Parse one line, given the current local-label scope.
fn parse_line(
    file: &str,
    line_no: usize,
    raw: &str,
    scope: &mut Option<String>,
    diagnostics: &mut Vec<Diagnostic>,
) -> ParsedLine {
    let text = raw.to_string();
    let stripped = strip_comment(raw);
    let mut label = None;
    let mut label_is_local = false;
    let mut sdas_name: Option<String> = None;
    let mut rest = stripped.trim();

    // A label is the text before the first top-level colon.
    if let Some(colon) = find_label_colon(stripped) {
        let name = stripped[..colon].trim();
        if !name.is_empty() {
            if name.contains(char::is_whitespace) {
                diagnostics.push(
                    Diagnostic::error(file, line_no, format!("malformed label `{name}`"))
                        .at(leading_columns(stripped), text.clone()),
                );
            } else {
                label_is_local = name.starts_with('.');
                let resolved = if label_is_local {
                    match scope {
                        Some(prefix) => format!("{prefix}{name}"),
                        None => name.to_string(),
                    }
                } else {
                    *scope = Some(name.to_string());
                    name.to_string()
                };
                label = Some(resolved);
            }
        }
        rest = stripped[colon + 1..].trim();
    }

    if rest.is_empty() {
        return ParsedLine {
            file: file.to_string(),
            line: line_no,
            text,
            label,
            label_is_local,
            statement: Statement::Empty,
        };
    }

    let (mut op_text, mut args_text) = match rest.find(char::is_whitespace) {
        Some(idx) => (&rest[..idx], rest[idx..].trim()),
        None => (rest, ""),
    };

    // sdas-style definitions put the symbol first: `TR0 .bit 0x8C`. If the
    // leading word is not a mnemonic but what follows is a directive, the
    // leading word is a label.
    if !op_text.starts_with('.')
        && Mnemonic::parse(op_text).is_none()
        && let Some(first) = args_text.split_whitespace().next()
        && first.starts_with('.')
    {
        let name = op_text.to_string();
        let is_local = name.starts_with('.');
        let resolved = if is_local {
            match scope {
                Some(prefix) => format!("{prefix}{name}"),
                None => name.clone(),
            }
        } else {
            *scope = Some(name.clone());
            name.clone()
        };
        if label.is_none() {
            label = Some(resolved);
            label_is_local = is_local;
            sdas_name = Some(name);
        }
        let tail = args_text;
        let (op, args) = match tail.find(char::is_whitespace) {
            Some(idx) => (&tail[..idx], tail[idx..].trim()),
            None => (tail, ""),
        };
        op_text = op;
        args_text = args;
    }

    let mut statement = if let Some(directive) = op_text.strip_prefix('.') {
        parse_directive(
            file,
            line_no,
            directive,
            args_text,
            &text,
            sdas_name.as_deref(),
            diagnostics,
        )
    } else {
        match Mnemonic::parse(op_text) {
            Some(mnemonic) => {
                let operands = if args_text.is_empty() {
                    Vec::new()
                } else {
                    split_operands(args_text)
                        .into_iter()
                        .map(|o| {
                            let parsed = parse_operand(&o);
                            if let OperandText::Bad(bad) = &parsed {
                                diagnostics.push(
                                    Diagnostic::error(
                                        file,
                                        line_no,
                                        format!("cannot parse operand `{bad}`"),
                                    )
                                    .at(leading_columns(stripped), text.clone()),
                                );
                            }
                            parsed
                        })
                        .collect()
                };
                Statement::Instruction { mnemonic, operands }
            }
            None => {
                diagnostics.push(
                    Diagnostic::error(file, line_no, format!("unknown mnemonic `{op_text}`"))
                        .at(leading_columns(stripped), text.clone()),
                );
                Statement::Unknown {
                    directive: op_text.to_string(),
                    args: args_text.to_string(),
                }
            }
        }
    };

    scope_statement(&mut statement, scope);

    ParsedLine {
        file: file.to_string(),
        line: line_no,
        text,
        label,
        label_is_local,
        statement,
    }
}

fn leading_columns(line: &str) -> usize {
    line.len() - line.trim_start().len()
}

/// Find the colon that terminates a label, ignoring string literals.
fn find_label_colon(line: &str) -> Option<usize> {
    let mut quote: Option<char> = None;
    for (i, c) in line.char_indices() {
        match quote {
            Some(q) => {
                if c == q {
                    quote = None;
                }
            }
            None => match c {
                '"' | '\'' => quote = Some(c),
                ':' => return Some(i),
                // A colon after whitespace-separated code is not a label.
                _ => {}
            },
        }
    }
    None
}

#[allow(clippy::too_many_arguments)]
fn parse_directive(
    file: &str,
    line_no: usize,
    directive: &str,
    args_text: &str,
    source: &str,
    sdas_name: Option<&str>,
    diagnostics: &mut Vec<Diagnostic>,
) -> Statement {
    let directive_lower = directive.to_ascii_lowercase();
    let args = split_operands(args_text);
    let mut expr_arg = |index: usize| -> Option<Expr> {
        let text = args.get(index)?;
        match expr::parse(text) {
            Some(e) => Some(e),
            None => {
                diagnostics.push(
                    Diagnostic::error(file, line_no, format!("cannot parse expression `{text}`"))
                        .at(leading_columns(source), source.to_string()),
                );
                None
            }
        }
    };

    match directive_lower.as_str() {
        "db" | "byte" | "ascii" | "asciz" => {
            let mut items = Vec::new();
            for arg in &args {
                if arg.is_empty() {
                    continue;
                }
                if (arg.starts_with('"') && arg.ends_with('"') && arg.len() >= 2)
                    || (arg.starts_with('\'') && arg.ends_with('\'') && arg.len() >= 2)
                {
                    let inner = &arg[1..arg.len() - 1];
                    let mut bytes = unescape(inner);
                    if directive_lower == "asciz" {
                        bytes.push(0);
                    }
                    items.push(DataItem::Text(bytes));
                } else {
                    match expr::parse(arg) {
                        Some(e) => items.push(DataItem::Value(e)),
                        None => diagnostics.push(
                            Diagnostic::error(
                                file,
                                line_no,
                                format!("cannot parse data value `{arg}`"),
                            )
                            .at(leading_columns(source), source.to_string()),
                        ),
                    }
                }
            }
            Statement::Data { width: 1, items }
        }
        "dw" | "word" => {
            let mut items = Vec::new();
            for arg in &args {
                if arg.is_empty() {
                    continue;
                }
                match expr::parse(arg) {
                    Some(e) => items.push(DataItem::Value(e)),
                    None => diagnostics.push(
                        Diagnostic::error(
                            file,
                            line_no,
                            format!("cannot parse data value `{arg}`"),
                        )
                        .at(leading_columns(source), source.to_string()),
                    ),
                }
            }
            Statement::Data { width: 2, items }
        }
        "ds" | "space" | "blkb" => {
            let count = expr_arg(0).unwrap_or(Expr::Number(0));
            let fill = if args.len() > 1 { expr_arg(1) } else { None };
            Statement::Reserve { count, fill }
        }
        "org" => match expr_arg(0) {
            Some(e) => Statement::Org(e),
            None => Statement::Unknown {
                directive: directive.to_string(),
                args: args_text.to_string(),
            },
        },
        "align" | "even" => {
            let value = if directive_lower == "even" {
                Some(Expr::Number(2))
            } else {
                expr_arg(0)
            };
            match value {
                Some(e) => Statement::Align(e),
                None => Statement::Unknown {
                    directive: directive.to_string(),
                    args: args_text.to_string(),
                },
            }
        }
        "equ" | "set" | "bit" | "sfr" | "data" | "gset" => {
            let kind = match directive_lower.as_str() {
                "bit" => SymbolKind::Bit,
                "sfr" => SymbolKind::Sfr,
                "data" => SymbolKind::Label,
                _ => SymbolKind::Equate,
            };
            // `NAME .bit 0x8C` puts the name before the directive; otherwise
            // the name is the first argument.
            let (name, value_index) = match sdas_name {
                Some(name) => (name.to_string(), 0),
                None => match args.first() {
                    Some(name) => (name.clone(), 1),
                    None => {
                        diagnostics.push(
                            Diagnostic::error(file, line_no, "directive needs a symbol name")
                                .at(leading_columns(source), source.to_string()),
                        );
                        return Statement::Empty;
                    }
                },
            };
            let Some(value) = expr_arg(value_index) else {
                return Statement::Empty;
            };
            Statement::Define {
                name,
                expr: value,
                kind,
            }
        }
        "globl" | "global" | "export" => {
            Statement::Globals(args.iter().filter(|a| !a.is_empty()).cloned().collect())
        }
        "extern" | "import" => {
            Statement::Externs(args.iter().filter(|a| !a.is_empty()).cloned().collect())
        }
        "entry" => match args.first() {
            Some(name) => Statement::Entry(name.clone()),
            None => Statement::Empty,
        },
        "end" => Statement::End,
        "area" | "module" | "bank" => {
            let joined = args.join(" ");
            let upper = joined.to_ascii_uppercase();
            let absolute = upper.contains("(ABS)") || upper.contains("ABS,") || upper == "ABS";
            let name = joined
                .split_whitespace()
                .next()
                .unwrap_or("CODE")
                .trim_matches(|c| c == '(' || c == ')')
                .to_string();
            Statement::Area {
                space: space_for_area(&name),
                name,
                absolute,
            }
        }
        _ => Statement::Unknown {
            directive: directive.to_string(),
            args: args_text.to_string(),
        },
    }
}

/// Expand the escape sequences the assembler understands inside string literals
/// and `.db` character constants.
///
/// Supported: `\n \r \t \0 \\ \" \'` and `\xNN` (one or two hex digits).
/// Anything else after a backslash is taken literally, so `.db "C:\path"` keeps
/// its backslash.
fn unescape(text: &str) -> Vec<u8> {
    let chars: Vec<char> = text.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c != '\\' {
            let mut buf = [0u8; 4];
            out.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
            i += 1;
            continue;
        }
        i += 1;
        let Some(escape) = chars.get(i).copied() else {
            out.push(b'\\');
            break;
        };
        i += 1;
        match escape {
            'n' => out.push(b'\n'),
            'r' => out.push(b'\r'),
            't' => out.push(b'\t'),
            '0' => out.push(0),
            '\\' => out.push(b'\\'),
            '"' => out.push(b'"'),
            '\'' => out.push(b'\''),
            'x' | 'X' => {
                // One or two hex digits; a lone `\x` is kept literally.
                let mut value: u32 = 0;
                let mut digits = 0;
                while digits < 2 {
                    match chars.get(i).and_then(|c| c.to_digit(16)) {
                        Some(digit) => {
                            value = value * 16 + digit;
                            i += 1;
                            digits += 1;
                        }
                        None => break,
                    }
                }
                if digits == 0 {
                    out.push(b'x');
                } else {
                    out.push(value as u8);
                }
            }
            other => {
                // An unknown escape keeps its backslash rather than silently
                // dropping it, so `"C:\path"` survives a round trip.
                out.push(b'\\');
                let mut buf = [0u8; 4];
                out.extend_from_slice(other.encode_utf8(&mut buf).as_bytes());
            }
        }
    }
    out
}

/// Rewrite references to local labels (`.name`) so they match the definition,
/// which is stored as `<scope>.name`.
fn scope_expr(expr: &mut Expr, scope: &Option<String>) {
    match expr {
        Expr::Symbol(name) => {
            if name.starts_with('.')
                && let Some(prefix) = scope
            {
                *name = format!("{prefix}{name}");
            }
        }
        Expr::Unary(_, inner) | Expr::Low(inner) | Expr::High(inner) => scope_expr(inner, scope),
        Expr::Binary(_, lhs, rhs) => {
            scope_expr(lhs, scope);
            scope_expr(rhs, scope);
        }
        Expr::Number(_) | Expr::Current => {}
    }
}

fn scope_operand(operand: &mut OperandText, scope: &Option<String>) {
    match operand {
        OperandText::Imm(e) | OperandText::NotBit(e) | OperandText::Bare(e) => scope_expr(e, scope),
        _ => {}
    }
}

fn scope_statement(statement: &mut Statement, scope: &Option<String>) {
    match statement {
        Statement::Instruction { operands, .. } => {
            for operand in operands {
                scope_operand(operand, scope);
            }
        }
        Statement::Data { items, .. } => {
            for item in items {
                if let DataItem::Value(e) = item {
                    scope_expr(e, scope);
                }
            }
        }
        Statement::Reserve { count, fill } => {
            scope_expr(count, scope);
            if let Some(fill) = fill {
                scope_expr(fill, scope);
            }
        }
        Statement::Org(e) | Statement::Align(e) => scope_expr(e, scope),
        Statement::Define { expr, .. } => scope_expr(expr, scope),
        _ => {}
    }
}

/// Parse a whole source unit, splicing in `.include`d files.
pub fn parse_source(
    source: &str,
    filename: &str,
    options: &Options,
    diagnostics: &mut Vec<Diagnostic>,
) -> Vec<ParsedLine> {
    let mut out = Vec::new();
    let mut scope = None;
    let mut stack = Vec::new();
    expand(
        source,
        filename,
        options,
        &mut out,
        &mut scope,
        diagnostics,
        &mut stack,
    );
    out
}

#[allow(clippy::too_many_arguments)]
fn expand(
    source: &str,
    filename: &str,
    options: &Options,
    out: &mut Vec<ParsedLine>,
    scope: &mut Option<String>,
    diagnostics: &mut Vec<Diagnostic>,
    stack: &mut Vec<PathBuf>,
) {
    for (i, raw) in source.lines().enumerate() {
        let line_no = i + 1;
        let parsed = parse_line(filename, line_no, raw, scope, diagnostics);
        if let Statement::Unknown { directive, args } = &parsed.statement {
            if directive.eq_ignore_ascii_case("include") {
                let raw_name = args.trim().trim_matches('"').trim_matches('\'');
                match load_include(raw_name, filename, options, stack) {
                    Ok((text, resolved)) => {
                        stack.push(PathBuf::from(&resolved));
                        expand(&text, &resolved, options, out, scope, diagnostics, stack);
                        stack.pop();
                    }
                    Err(message) => diagnostics.push(
                        Diagnostic::error(filename, line_no, message)
                            .at(leading_columns(raw), raw.to_string()),
                    ),
                }
                continue;
            }
            if !directive.is_empty() {
                diagnostics.push(
                    Diagnostic::error(
                        filename,
                        line_no,
                        format!("unknown directive `.{directive}`"),
                    )
                    .at(leading_columns(raw), raw.to_string()),
                );
            }
        }
        out.push(parsed);
    }
}

fn load_include(
    name: &str,
    from: &str,
    options: &Options,
    stack: &[PathBuf],
) -> Result<(String, String), String> {
    if let Some((_, contents)) = options.virtual_includes.iter().find(|(key, _)| key == name) {
        return Ok((contents.clone(), format!("<virtual:{name}>")));
    }
    let mut candidates: Vec<PathBuf> = Vec::new();
    let requested = Path::new(name);
    if requested.is_absolute() {
        candidates.push(requested.to_path_buf());
    } else {
        if let Some(dir) = Path::new(from).parent() {
            candidates.push(dir.join(requested));
        }
        for dir in &options.include_paths {
            candidates.push(dir.join(requested));
        }
        candidates.push(PathBuf::from(requested));
    }
    for candidate in &candidates {
        if stack.iter().any(|p| p == candidate) {
            return Err(format!("recursive include of `{name}`"));
        }
        if let Ok(text) = std::fs::read_to_string(candidate) {
            return Ok((text, candidate.display().to_string()));
        }
    }
    if let Some(text) = builtin_include(name) {
        return Ok((text.to_string(), format!("<builtin:{name}>")));
    }
    Err(format!(
        "cannot find include `{name}` (searched {})",
        candidates
            .iter()
            .map(|c| c.display().to_string())
            .collect::<Vec<_>>()
            .join(", ")
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_comments_outside_strings() {
        assert_eq!(strip_comment("MOV A,#1 ; hi").trim(), "MOV A,#1");
        assert_eq!(strip_comment("MOV A,#1 // hi").trim(), "MOV A,#1");
        assert_eq!(strip_comment(".db \"a;b\"").trim(), ".db \"a;b\"");
    }

    #[test]
    fn splits_operands_respecting_parens_and_strings() {
        assert_eq!(split_operands("A,#0x30"), vec!["A", "#0x30"]);
        assert_eq!(split_operands("\"a,b\",1"), vec!["\"a,b\"", "1"]);
        assert_eq!(split_operands("low(x+1),2"), vec!["low(x+1)", "2"]);
    }

    #[test]
    fn recognises_operand_forms() {
        assert_eq!(parse_operand("A"), OperandText::Acc);
        assert_eq!(parse_operand(" r3 "), OperandText::Rn(3));
        assert_eq!(parse_operand("@R1"), OperandText::AtRi(1));
        assert_eq!(parse_operand("@A+DPTR"), OperandText::AtADptr);
        assert!(matches!(parse_operand("#5"), OperandText::Imm(_)));
        assert!(matches!(parse_operand("/TR0"), OperandText::NotBit(_)));
        assert!(matches!(parse_operand("label+1"), OperandText::Bare(_)));
    }

    #[test]
    fn parses_a_label_and_instruction() {
        let options = Options::new("t.asm");
        let mut diags = Vec::new();
        let lines = parse_source("start:  MOV A, #1\n", "t.asm", &options, &mut diags);
        assert!(diags.is_empty(), "{diags:?}");
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].label.as_deref(), Some("start"));
        assert!(matches!(
            lines[0].statement,
            Statement::Instruction {
                mnemonic: Mnemonic::Mov,
                ..
            }
        ));
    }

    #[test]
    fn local_labels_are_scoped() {
        let options = Options::new("t.asm");
        let mut diags = Vec::new();
        let lines = parse_source(
            "global:\n.loop: NOP\nglobal2:\n.loop: NOP\n",
            "t.asm",
            &options,
            &mut diags,
        );
        assert_eq!(lines[1].label.as_deref(), Some("global.loop"));
        assert_eq!(lines[3].label.as_deref(), Some("global2.loop"));
    }
}
