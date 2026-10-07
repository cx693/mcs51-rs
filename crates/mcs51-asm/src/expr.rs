//! Constant and relocatable expressions.

use std::fmt;

/// A symbol as seen by the expression evaluator.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Resolved {
    /// A plain number.
    Absolute(i64),
    /// A label at a known absolute address.
    Located(i64),
    /// A label in a relocatable section.
    Relocatable {
        /// Section the label lives in.
        section: String,
        /// Byte offset within the section.
        offset: u32,
    },
}

/// How a relocatable value is narrowed when it is written out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Transform {
    /// The value itself.
    Plain,
    /// The low byte, from `low()`.
    Low,
    /// The high byte, from `high()`.
    High,
}

/// The result of evaluating an expression.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Value {
    /// A number known at assembly time.
    Abs(i64),
    /// An address that only the linker can compute.
    Reloc {
        /// Symbol the value is based on.
        symbol: String,
        /// Constant added to the symbol.
        addend: i64,
        /// Section the symbol lives in, when known.
        section: Option<String>,
        /// Offset of the symbol within that section, when known.
        offset: Option<u32>,
        /// Narrowing applied by `low()`/`high()`.
        transform: Transform,
    },
}

impl Value {
    /// The value as a number, if it is absolute.
    pub fn as_abs(&self) -> Option<i64> {
        match self {
            Value::Abs(v) => Some(*v),
            _ => None,
        }
    }

    /// `true` when the linker still has to resolve this.
    pub fn is_reloc(&self) -> bool {
        matches!(self, Value::Reloc { .. })
    }
}

impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Value::Abs(v) => write!(f, "{v}"),
            Value::Reloc {
                symbol,
                addend,
                transform,
                ..
            } => {
                let name = match transform {
                    Transform::Plain => "",
                    Transform::Low => "low(",
                    Transform::High => "high(",
                };
                write!(f, "{name}{symbol}{addend:+})")
            }
        }
    }
}

/// An unevaluated expression.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Expr {
    /// A literal number.
    Number(i64),
    /// A symbol reference.
    Symbol(String),
    /// `$` — the current location counter.
    Current,
    /// Unary operator.
    Unary(UnOp, Box<Expr>),
    /// Binary operator.
    Binary(BinOp, Box<Expr>, Box<Expr>),
    /// `low(x)`
    Low(Box<Expr>),
    /// `high(x)`
    High(Box<Expr>),
}

/// Unary operators.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(missing_docs)]
pub enum UnOp {
    Neg,
    Not,
    Plus,
}

/// Binary operators.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(missing_docs)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    Shl,
    Shr,
    And,
    Or,
    Xor,
}

/// Why an expression could not be evaluated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExprError {
    /// A symbol was not defined anywhere.
    UndefinedSymbol(String),
    /// The expression needs the linker but is used where a constant is required.
    NotConstant(String),
    /// A subtraction mixes symbols from different sections.
    CrossSectionSubtract {
        /// Left-hand symbol.
        left: String,
        /// Right-hand symbol.
        right: String,
    },
    /// Division or modulo by zero.
    DivideByZero,
    /// The value does not fit the 8051's 16-bit address space.
    Overflow(i64),
}

impl fmt::Display for ExprError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ExprError::UndefinedSymbol(name) => write!(f, "undefined symbol `{name}`"),
            ExprError::NotConstant(text) => {
                write!(f, "`{text}` is not a constant here")
            }
            ExprError::CrossSectionSubtract { left, right } => write!(
                f,
                "cannot subtract `{right}` from `{left}`: they are in different sections"
            ),
            ExprError::DivideByZero => f.write_str("division by zero"),
            ExprError::Overflow(v) => write!(f, "value {v} does not fit in 16 bits"),
        }
    }
}

impl std::error::Error for ExprError {}

impl Expr {
    /// Evaluate against a symbol resolver and the current location counter.
    pub fn eval(
        &self,
        resolve: &dyn Fn(&str) -> Option<Resolved>,
        here: i64,
    ) -> Result<Value, ExprError> {
        match self {
            Expr::Number(n) => Ok(Value::Abs(*n)),
            Expr::Current => Ok(Value::Abs(here)),
            Expr::Symbol(name) => match resolve(name) {
                Some(Resolved::Absolute(v)) | Some(Resolved::Located(v)) => Ok(Value::Abs(v)),
                Some(Resolved::Relocatable { section, offset }) => Ok(Value::Reloc {
                    symbol: name.clone(),
                    addend: 0,
                    section: Some(section),
                    offset: Some(offset),
                    transform: Transform::Plain,
                }),
                None => Err(ExprError::UndefinedSymbol(name.clone())),
            },
            Expr::Unary(op, inner) => {
                let v = inner.eval(resolve, here)?.abs()?;
                Ok(Value::Abs(match op {
                    UnOp::Neg => v.wrapping_neg(),
                    UnOp::Not => !v,
                    UnOp::Plus => v,
                }))
            }
            Expr::Low(inner) => {
                let v = inner.eval(resolve, here)?;
                Ok(match v {
                    Value::Abs(n) => Value::Abs(n & 0xFF),
                    Value::Reloc {
                        symbol,
                        addend,
                        section,
                        offset,
                        ..
                    } => Value::Reloc {
                        symbol,
                        addend,
                        section,
                        offset,
                        transform: Transform::Low,
                    },
                })
            }
            Expr::High(inner) => {
                let v = inner.eval(resolve, here)?;
                Ok(match v {
                    Value::Abs(n) => Value::Abs((n >> 8) & 0xFF),
                    Value::Reloc {
                        symbol,
                        addend,
                        section,
                        offset,
                        ..
                    } => Value::Reloc {
                        symbol,
                        addend,
                        section,
                        offset,
                        transform: Transform::High,
                    },
                })
            }
            Expr::Binary(op, lhs, rhs) => {
                let l = lhs.eval(resolve, here)?;
                let r = rhs.eval(resolve, here)?;
                eval_binary(*op, l, r)
            }
        }
    }
}

fn eval_binary(op: BinOp, l: Value, r: Value) -> Result<Value, ExprError> {
    match op {
        BinOp::Add => match (l, r) {
            (Value::Abs(a), Value::Abs(b)) => Ok(Value::Abs(a.wrapping_add(b))),
            (
                Value::Reloc {
                    symbol,
                    addend,
                    section,
                    offset,
                    transform,
                },
                Value::Abs(b),
            )
            | (
                Value::Abs(b),
                Value::Reloc {
                    symbol,
                    addend,
                    section,
                    offset,
                    transform,
                },
            ) => Ok(Value::Reloc {
                symbol,
                addend: addend.wrapping_add(b),
                section,
                offset,
                transform,
            }),
            (Value::Reloc { .. }, Value::Reloc { .. }) => Err(ExprError::NotConstant(
                "sum of two relocatable symbols".into(),
            )),
        },
        BinOp::Sub => match (l, r) {
            (Value::Abs(a), Value::Abs(b)) => Ok(Value::Abs(a.wrapping_sub(b))),
            (
                Value::Reloc {
                    symbol,
                    addend,
                    section,
                    offset,
                    transform,
                },
                Value::Abs(b),
            ) => Ok(Value::Reloc {
                symbol,
                addend: addend.wrapping_sub(b),
                section,
                offset,
                transform,
            }),
            (
                Value::Reloc {
                    symbol: ls,
                    addend: la,
                    section: lsec,
                    offset: loff,
                    ..
                },
                Value::Reloc {
                    symbol: rs,
                    addend: ra,
                    section: rsec,
                    offset: roff,
                    ..
                },
            ) => match (lsec, rsec, loff, roff) {
                (Some(a), Some(b), Some(x), Some(y)) if a == b => {
                    Ok(Value::Abs((x as i64 + la) - (y as i64 + ra)))
                }
                _ => Err(ExprError::CrossSectionSubtract {
                    left: ls,
                    right: rs,
                }),
            },
            (Value::Abs(_), Value::Reloc { .. }) => {
                Err(ExprError::NotConstant("constant minus a symbol".into()))
            }
        },
        _ => {
            let a = l.abs()?;
            let b = r.abs()?;
            Ok(Value::Abs(match op {
                BinOp::Mul => a.wrapping_mul(b),
                BinOp::Div => {
                    if b == 0 {
                        return Err(ExprError::DivideByZero);
                    }
                    a.wrapping_div(b)
                }
                BinOp::Mod => {
                    if b == 0 {
                        return Err(ExprError::DivideByZero);
                    }
                    a.wrapping_rem(b)
                }
                BinOp::Shl => a.wrapping_shl((b & 63) as u32),
                BinOp::Shr => a.wrapping_shr((b & 63) as u32),
                BinOp::And => a & b,
                BinOp::Or => a | b,
                BinOp::Xor => a ^ b,
                BinOp::Add | BinOp::Sub => unreachable!("handled above"),
            }))
        }
    }
}

impl Value {
    fn abs(self) -> Result<i64, ExprError> {
        match self {
            Value::Abs(v) => Ok(v),
            other => Err(ExprError::NotConstant(other.to_string())),
        }
    }
}

// ---------------------------------------------------------------------------
// Parser
// ---------------------------------------------------------------------------

/// Parse an expression, returning `None` on a syntax error.
pub fn parse(text: &str) -> Option<Expr> {
    let mut p = Parser {
        chars: text.chars().collect(),
        pos: 0,
    };
    p.skip_space();
    let expr = p.parse_binary(0)?;
    p.skip_space();
    if p.pos != p.chars.len() {
        return None;
    }
    Some(expr)
}

struct Parser {
    chars: Vec<char>,
    pos: usize,
}

/// Binding power of an infix operator, or `None` if it is not one.
fn infix_op(c: char) -> Option<(BinOp, u8)> {
    Some(match c {
        '|' => (BinOp::Or, 1),
        '^' => (BinOp::Xor, 2),
        '&' => (BinOp::And, 3),
        '<' => (BinOp::Shl, 4),
        '>' => (BinOp::Shr, 4),
        '+' => (BinOp::Add, 5),
        '-' => (BinOp::Sub, 5),
        '*' => (BinOp::Mul, 6),
        '/' => (BinOp::Div, 6),
        '%' => (BinOp::Mod, 6),
        _ => return None,
    })
}

impl Parser {
    fn peek(&self) -> Option<char> {
        self.chars.get(self.pos).copied()
    }

    fn bump(&mut self) -> Option<char> {
        let c = self.peek();
        if c.is_some() {
            self.pos += 1;
        }
        c
    }

    fn skip_space(&mut self) {
        while matches!(self.peek(), Some(c) if c.is_whitespace()) {
            self.pos += 1;
        }
    }

    fn parse_binary(&mut self, min_bp: u8) -> Option<Expr> {
        let mut lhs = self.parse_unary()?;
        loop {
            self.skip_space();
            let Some(c) = self.peek() else { break };
            let Some((op, bp)) = infix_op(c) else { break };
            if bp < min_bp {
                break;
            }
            self.pos += 1;
            // `<<` and `>>` are two-character operators.
            if matches!(op, BinOp::Shl | BinOp::Shr) {
                if self.peek() != Some(c) {
                    return None;
                }
                self.pos += 1;
            }
            self.skip_space();
            let rhs = self.parse_binary(bp + 1)?;
            lhs = Expr::Binary(op, Box::new(lhs), Box::new(rhs));
        }
        Some(lhs)
    }

    fn parse_unary(&mut self) -> Option<Expr> {
        self.skip_space();
        match self.peek() {
            Some('-') => {
                self.pos += 1;
                Some(Expr::Unary(UnOp::Neg, Box::new(self.parse_unary()?)))
            }
            Some('~') => {
                self.pos += 1;
                Some(Expr::Unary(UnOp::Not, Box::new(self.parse_unary()?)))
            }
            Some('+') => {
                self.pos += 1;
                Some(Expr::Unary(UnOp::Plus, Box::new(self.parse_unary()?)))
            }
            _ => self.parse_atom(),
        }
    }

    fn parse_atom(&mut self) -> Option<Expr> {
        self.skip_space();
        let c = self.peek()?;
        match c {
            '(' => {
                self.pos += 1;
                let inner = self.parse_binary(0)?;
                self.skip_space();
                if self.bump() != Some(')') {
                    return None;
                }
                Some(inner)
            }
            '\'' => {
                self.pos += 1;
                let value = match self.bump()? {
                    '\\' => {
                        let esc = self.bump()?;
                        match esc {
                            'n' => 0x0A,
                            'r' => 0x0D,
                            't' => 0x09,
                            '0' => 0x00,
                            '\\' => 0x5C,
                            '\'' => 0x27,
                            '"' => 0x22,
                            other => other as i64,
                        }
                    }
                    other => other as i64,
                };
                if self.bump() != Some('\'') {
                    return None;
                }
                Some(Expr::Number(value & 0xFF))
            }
            '$' => {
                self.pos += 1;
                Some(Expr::Current)
            }
            c if c.is_ascii_digit() => self.parse_number(),
            c if c.is_alphabetic() || c == '_' || c == '.' || c == '@' => {
                let name = self.parse_ident();
                let upper = name.to_ascii_uppercase();
                if upper == "LOW" || upper == "HIGH" {
                    self.skip_space();
                    if self.bump() != Some('(') {
                        return None;
                    }
                    let inner = self.parse_binary(0)?;
                    self.skip_space();
                    if self.bump() != Some(')') {
                        return None;
                    }
                    return Some(if upper == "LOW" {
                        Expr::Low(Box::new(inner))
                    } else {
                        Expr::High(Box::new(inner))
                    });
                }
                Some(Expr::Symbol(name))
            }
            _ => None,
        }
    }

    fn parse_ident(&mut self) -> String {
        let mut out = String::new();
        while let Some(c) = self.peek() {
            if c.is_alphanumeric() || c == '_' || c == '.' || c == '@' || c == '?' {
                out.push(c);
                self.pos += 1;
            } else {
                break;
            }
        }
        out
    }

    /// Parse a numeric literal.
    ///
    /// Accepts C-style `0x1F` and `0b1010`, the assembler-traditional `1Fh` and
    /// `1010b` suffixes, and plain decimal.
    fn parse_number(&mut self) -> Option<Expr> {
        let mut token = String::new();
        while let Some(c) = self.peek() {
            if c.is_ascii_alphanumeric() || c == '_' {
                if c != '_' {
                    token.push(c);
                }
                self.pos += 1;
            } else {
                break;
            }
        }
        let lower = token.to_ascii_lowercase();
        let (radix, digits) = if let Some(rest) = lower.strip_prefix("0x") {
            (16, rest.to_string())
        } else if let Some(rest) = lower.strip_prefix("0b") {
            (2, rest.to_string())
        } else if let Some(rest) = lower.strip_suffix('h') {
            (16, rest.to_string())
        } else if let Some(rest) = lower.strip_suffix('b') {
            (2, rest.to_string())
        } else {
            (10, lower.clone())
        };
        if digits.is_empty() || !digits.chars().all(|c| c.is_digit(radix)) {
            return None;
        }
        i64::from_str_radix(&digits, radix).ok().map(Expr::Number)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn eval_abs(text: &str) -> i64 {
        let expr = parse(text).unwrap_or_else(|| panic!("cannot parse {text}"));
        expr.eval(&|_| None, 0)
            .unwrap_or_else(|e| panic!("cannot evaluate {text}: {e}"))
            .as_abs()
            .unwrap()
    }

    #[test]
    fn arithmetic() {
        assert_eq!(eval_abs("1 + 2 * 3"), 7);
        assert_eq!(eval_abs("(1 + 2) * 3"), 9);
        assert_eq!(eval_abs("0x10 | 0x01"), 0x11);
        assert_eq!(eval_abs("1 << 4"), 16);
        assert_eq!(eval_abs("0xFF & 0x0F"), 0x0F);
        assert_eq!(eval_abs("~0"), -1);
        assert_eq!(eval_abs("-5 + 10"), 5);
        assert_eq!(eval_abs("'A'"), 65);
        assert_eq!(eval_abs("10 / 3"), 3);
        assert_eq!(eval_abs("10 % 3"), 1);
    }

    #[test]
    fn alternative_number_bases() {
        assert_eq!(eval_abs("0FFh"), 0xFF);
        assert_eq!(eval_abs("1010b"), 10);
        assert_eq!(eval_abs("0b1010"), 10);
    }

    #[test]
    fn low_and_high() {
        assert_eq!(eval_abs("low(0x1234)"), 0x34);
        assert_eq!(eval_abs("high(0x1234)"), 0x12);
    }

    #[test]
    fn symbol_difference_is_absolute() {
        let symbol = |name: &str| match name {
            "a" => Some(Resolved::Relocatable {
                section: "CODE".into(),
                offset: 0x10,
            }),
            "b" => Some(Resolved::Relocatable {
                section: "CODE".into(),
                offset: 0x40,
            }),
            _ => None,
        };
        let expr = parse("b - a").unwrap();
        assert_eq!(expr.eval(&symbol, 0).unwrap().as_abs(), Some(0x30));

        let expr = parse("b + 4").unwrap();
        let value = expr.eval(&symbol, 0).unwrap();
        assert!(value.is_reloc());
    }

    #[test]
    fn rejects_cross_section_subtraction() {
        let symbol = |name: &str| match name {
            "a" => Some(Resolved::Relocatable {
                section: "CODE".into(),
                offset: 0,
            }),
            "b" => Some(Resolved::Relocatable {
                section: "DATA".into(),
                offset: 0,
            }),
            _ => None,
        };
        let expr = parse("b - a").unwrap();
        assert!(matches!(
            expr.eval(&symbol, 0),
            Err(ExprError::CrossSectionSubtract { .. })
        ));
    }

    #[test]
    fn rejects_nonsense() {
        assert!(parse("1 +").is_none());
        assert!(parse("(1").is_none());
        assert!(parse("1 2").is_none());
    }
}
