//! Expression lexer, parser and evaluator.
//!
//! One AST serves both real arithmetic (f64) and programming mode
//! (word-size-masked two's-complement integers).

use std::collections::HashMap;
use std::fmt;

use crate::financial;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AngleUnit {
    Degrees,
    Radians,
    Gradians,
}

impl AngleUnit {
    pub fn nick(self) -> &'static str {
        match self {
            AngleUnit::Degrees => "degrees",
            AngleUnit::Radians => "radians",
            AngleUnit::Gradians => "gradians",
        }
    }
    pub fn from_nick(s: &str) -> Self {
        match s {
            "radians" => AngleUnit::Radians,
            "gradians" => AngleUnit::Gradians,
            _ => AngleUnit::Degrees,
        }
    }
    fn to_radians(self, x: f64) -> f64 {
        match self {
            AngleUnit::Degrees => x.to_radians(),
            AngleUnit::Radians => x,
            AngleUnit::Gradians => x * std::f64::consts::PI / 200.0,
        }
    }
    fn radians_to_unit(self, x: f64) -> f64 {
        match self {
            AngleUnit::Degrees => x.to_degrees(),
            AngleUnit::Radians => x,
            AngleUnit::Gradians => x * 200.0 / std::f64::consts::PI,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum CalcError {
    Syntax { pos: usize, msg: String },
    DivisionByZero,
    Domain(String),
    UnknownVariable(String),
    UnknownFunction(String),
    Arity { func: String, expected: usize },
    NotInteger,
    Overflow,
    Empty,
}

impl fmt::Display for CalcError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CalcError::Syntax { msg, .. } => write!(f, "{msg}"),
            CalcError::DivisionByZero => write!(f, "Division by zero is undefined"),
            CalcError::Domain(m) => write!(f, "{m}"),
            CalcError::UnknownVariable(v) => write!(f, "Unknown variable “{v}”"),
            CalcError::UnknownFunction(v) => write!(f, "Function “{v}” is not defined"),
            CalcError::Arity { func, expected } => {
                write!(
                    f,
                    "{func} takes {expected} argument{}",
                    if *expected == 1 { "" } else { "s" }
                )
            }
            CalcError::NotInteger => write!(f, "Programming mode only supports integers"),
            CalcError::Overflow => write!(f, "Overflow: the result is too large"),
            CalcError::Empty => write!(f, "No expression"),
        }
    }
}

impl CalcError {
    /// Character position of a syntax error, if known.
    pub fn position(&self) -> Option<usize> {
        match self {
            CalcError::Syntax { pos, .. } => Some(*pos),
            _ => None,
        }
    }
}

type Result<T> = std::result::Result<T, CalcError>;

// ---------------------------------------------------------------- lexer

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Num {
        digits: String,
        base: Option<u32>,
    },
    Ident(String),
    Plus,
    Minus,
    Mul,
    Div,
    Pow,
    Bang,
    Percent,
    Sqrt,
    Cbrt,
    LParen,
    RParen,
    Pipe,
    Sep,
    Assign,
    Shl,
    Shr,
    /// Superscript exponent, e.g. `²` → "2", `⁻¹` → "-1".
    Sup(String),
    Pi,
}

#[derive(Debug, Clone)]
struct Token {
    tok: Tok,
    pos: usize,
}

pub const FUNCTIONS: &[&str] = &[
    "sin", "cos", "tan", "asin", "acos", "atan", "sinh", "cosh", "tanh", "asinh", "acosh", "atanh",
    "ln", "log", "log2", "exp", "sqrt", "cbrt", "abs", "int", "frac", "round", "floor", "ceil",
    "sgn", "ones", "twos", "ctrm", "ddb", "fv", "gpm", "pmt", "pv", "rate", "sln", "syd", "term",
];

const KEYWORDS: &[&str] = &["mod", "and", "or", "xor", "not"];

fn sup_digit(c: char) -> Option<char> {
    Some(match c {
        '⁰' => '0',
        '¹' => '1',
        '²' => '2',
        '³' => '3',
        '⁴' => '4',
        '⁵' => '5',
        '⁶' => '6',
        '⁷' => '7',
        '⁸' => '8',
        '⁹' => '9',
        '⁻' => '-',
        _ => return None,
    })
}

fn sub_digit(c: char) -> Option<char> {
    let d = (c as u32).checked_sub('₀' as u32)?;
    if d < 10 {
        char::from_digit(d, 10)
    } else {
        None
    }
}

fn is_ident_char(c: char) -> bool {
    c.is_alphabetic() && c != 'π' || c == '_'
}

struct Lexer<'a> {
    chars: Vec<char>,
    i: usize,
    default_base: u32,
    vars: &'a HashMap<String, f64>,
}

impl<'a> Lexer<'a> {
    fn err<T>(&self, pos: usize, msg: &str) -> Result<T> {
        Err(CalcError::Syntax {
            pos,
            msg: msg.into(),
        })
    }

    fn tokens(mut self) -> Result<Vec<Token>> {
        let mut out: Vec<Token> = Vec::new();
        // Stack of paren kinds: true = function-call parens.
        let mut parens: Vec<bool> = Vec::new();
        while self.i < self.chars.len() {
            let c = self.chars[self.i];
            let pos = self.i;
            if c.is_whitespace() {
                self.i += 1;
                continue;
            }
            let in_call = parens.last().copied().unwrap_or(false);
            let tok = if c.is_ascii_digit() || c == '.' {
                self.number(in_call)?
            } else if is_ident_char(c) {
                let start = self.i;
                while self.i < self.chars.len()
                    && (is_ident_char(self.chars[self.i]) || self.chars[self.i].is_ascii_digit())
                {
                    self.i += 1;
                }
                let word: String = self.chars[start..self.i].iter().collect();
                if self.peek().and_then(sub_digit).is_some()
                    && word.chars().all(|c| c.is_ascii_hexdigit())
                {
                    let base = self.subscript_base();
                    out.push(Token {
                        tok: Tok::Num {
                            digits: word.to_uppercase(),
                            base,
                        },
                        pos,
                    });
                    continue;
                }
                // In hexadecimal mode a word made only of hex digits that is
                // not a known name is a number.
                let lower = word.to_lowercase();
                if self.default_base == 16
                    && word.chars().all(|c| c.is_ascii_hexdigit())
                    && !self.vars.contains_key(&word)
                    && !FUNCTIONS.contains(&lower.as_str())
                    && !KEYWORDS.contains(&lower.as_str())
                {
                    let mut digits = word;
                    // Allow trailing subscript base.
                    let base = self.subscript_base();
                    digits.make_ascii_uppercase();
                    Tok::Num { digits, base }
                } else {
                    Tok::Ident(word)
                }
            } else if let Some(d) = sup_digit(c) {
                let mut s = String::new();
                while self.i < self.chars.len() {
                    match sup_digit(self.chars[self.i]) {
                        Some(d) => s.push(d),
                        None => break,
                    }
                    self.i += 1;
                }
                if s == "-" {
                    return self.err(pos, "Malformed exponent");
                }
                let _ = d;
                Tok::Sup(s)
            } else {
                self.i += 1;
                match c {
                    '+' => Tok::Plus,
                    '-' | '−' => Tok::Minus,
                    '*' | '×' | '·' | '⋅' => Tok::Mul,
                    '/' | '÷' | '∕' => Tok::Div,
                    '^' => Tok::Pow,
                    '!' => Tok::Bang,
                    '%' => Tok::Percent,
                    '√' => Tok::Sqrt,
                    '∛' => Tok::Cbrt,
                    'π' => Tok::Pi,
                    '(' => {
                        let call = matches!(out.last(), Some(Token { tok: Tok::Ident(n), .. })
                            if FUNCTIONS.contains(&n.to_lowercase().as_str()))
                            || matches!(out.last(), Some(Token { tok: Tok::Sup(_), .. })
                                if matches!(out.iter().rev().nth(1), Some(Token { tok: Tok::Ident(_), .. })));
                        parens.push(call);
                        Tok::LParen
                    }
                    ')' => {
                        parens.pop();
                        Tok::RParen
                    }
                    '|' => Tok::Pipe,
                    ';' | ',' => Tok::Sep,
                    '=' => Tok::Assign,
                    '<' if self.peek() == Some('<') => {
                        self.i += 1;
                        Tok::Shl
                    }
                    '>' if self.peek() == Some('>') => {
                        self.i += 1;
                        Tok::Shr
                    }
                    _ => return self.err(pos, &format!("Unexpected “{c}”")),
                }
            };
            out.push(Token { tok, pos });
        }
        Ok(out)
    }

    fn peek(&self) -> Option<char> {
        self.chars.get(self.i).copied()
    }

    fn subscript_base(&mut self) -> Option<u32> {
        let mut s = String::new();
        while let Some(d) = self.peek().and_then(sub_digit) {
            s.push(d);
            self.i += 1;
        }
        s.parse().ok()
    }

    fn number(&mut self, in_call: bool) -> Result<Tok> {
        let start = self.i;
        // Prefixed literals.
        if self.chars[self.i] == '0' {
            if let Some(p) = self.chars.get(self.i + 1).map(|c| c.to_ascii_lowercase()) {
                let base = match p {
                    'x' => Some(16),
                    'b' => Some(2),
                    'o' => Some(8),
                    _ => None,
                };
                if let Some(base) = base {
                    let digits_start = self.i + 2;
                    let mut j = digits_start;
                    while j < self.chars.len() && self.chars[j].is_digit(base) {
                        j += 1;
                    }
                    if j > digits_start {
                        self.i = j;
                        let digits: String = self.chars[digits_start..j]
                            .iter()
                            .collect::<String>()
                            .to_uppercase();
                        return Ok(Tok::Num {
                            digits,
                            base: Some(base),
                        });
                    }
                }
            }
        }
        // Literal with an explicit subscript base, e.g. 1F₁₆ or 101₂.
        {
            let mut j = self.i;
            while j < self.chars.len() && self.chars[j].is_ascii_hexdigit() {
                j += 1;
            }
            if self.chars.get(j).copied().and_then(sub_digit).is_some() {
                let digits: String = self.chars[self.i..j]
                    .iter()
                    .collect::<String>()
                    .to_uppercase();
                self.i = j;
                let base = self.subscript_base();
                return Ok(Tok::Num { digits, base });
            }
        }
        let radix = self.default_base;
        let mut digits = String::new();
        let mut seen_dot = false;
        while self.i < self.chars.len() {
            let c = self.chars[self.i];
            if c == '.' && !seen_dot && radix == 10 {
                seen_dot = true;
                digits.push('.');
                self.i += 1;
            } else if c.is_digit(radix) || (radix == 10 && c.is_ascii_digit()) {
                digits.push(c.to_ascii_uppercase());
                self.i += 1;
            } else if c == ',' && !in_call && !seen_dot && radix == 10 {
                // Thousands separator: exactly three digits must follow.
                let group: Vec<char> = self.chars[self.i + 1..].iter().take(4).copied().collect();
                if group.len() >= 3
                    && group[..3].iter().all(|c| c.is_ascii_digit())
                    && group.get(3).is_none_or(|c| !c.is_ascii_digit())
                {
                    self.i += 1;
                } else {
                    break;
                }
            } else {
                break;
            }
        }
        // Scientific notation: 1.5e3 / 1.5E-3 (decimal only).
        if radix == 10 && matches!(self.peek(), Some('e') | Some('E')) {
            let mut j = self.i + 1;
            if matches!(self.chars.get(j), Some('+') | Some('-')) {
                j += 1;
            }
            if self.chars.get(j).is_some_and(|c| c.is_ascii_digit()) {
                let exp_start = self.i;
                while j < self.chars.len() && self.chars[j].is_ascii_digit() {
                    j += 1;
                }
                digits.extend(&self.chars[exp_start..j]);
                self.i = j;
            }
        }
        if digits == "." {
            return self.err(start, "Malformed number");
        }
        let base = self.subscript_base();
        Ok(Tok::Num { digits, base })
    }
}

// ---------------------------------------------------------------- AST

#[derive(Debug, Clone, PartialEq)]
pub enum Expr {
    Num {
        digits: String,
        base: Option<u32>,
    },
    Const(f64),
    Var(String),
    Rand,
    Neg(Box<Expr>),
    Not(Box<Expr>),
    Bin(BinOp, Box<Expr>, Box<Expr>),
    Call {
        name: String,
        inverse: bool,
        power: Option<Box<Expr>>,
        args: Vec<Expr>,
    },
    Factorial(Box<Expr>),
    Percent(Box<Expr>),
    Root(Box<Expr>, Box<Expr>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    Pow,
    And,
    Or,
    Xor,
    Shl,
    Shr,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Statement {
    Expr(Expr),
    Assign(String, Expr),
}

struct Parser {
    toks: Vec<Token>,
    i: usize,
    end: usize,
}

impl Parser {
    fn peek(&self) -> Option<&Tok> {
        self.toks.get(self.i).map(|t| &t.tok)
    }
    fn pos(&self) -> usize {
        self.toks.get(self.i).map(|t| t.pos).unwrap_or(self.end)
    }
    fn bump(&mut self) -> Option<Tok> {
        let t = self.toks.get(self.i).map(|t| t.tok.clone());
        self.i += 1;
        t
    }
    fn eat(&mut self, t: &Tok) -> bool {
        if self.peek() == Some(t) {
            self.i += 1;
            true
        } else {
            false
        }
    }
    fn eat_kw(&mut self, kw: &str) -> bool {
        if matches!(self.peek(), Some(Tok::Ident(w)) if w.eq_ignore_ascii_case(kw)) {
            self.i += 1;
            true
        } else {
            false
        }
    }
    fn err<T>(&self, msg: &str) -> Result<T> {
        Err(CalcError::Syntax {
            pos: self.pos(),
            msg: msg.into(),
        })
    }

    fn statement(&mut self) -> Result<Statement> {
        if self.toks.is_empty() {
            return Err(CalcError::Empty);
        }
        if let (Some(Tok::Ident(name)), Some(Tok::Assign)) = (
            self.toks.first().map(|t| &t.tok),
            self.toks.get(1).map(|t| &t.tok),
        ) {
            let name = name.clone();
            if FUNCTIONS.contains(&name.to_lowercase().as_str())
                || KEYWORDS.contains(&name.to_lowercase().as_str())
                || is_builtin_const(&name)
            {
                return self.err(&format!("Cannot assign to “{name}”"));
            }
            self.i = 2;
            let e = self.expr()?;
            self.finish()?;
            return Ok(Statement::Assign(name, e));
        }
        let e = self.expr()?;
        self.finish()?;
        Ok(Statement::Expr(e))
    }

    fn finish(&self) -> Result<()> {
        if self.i < self.toks.len() {
            let msg = match self.peek() {
                Some(Tok::RParen) => "Unbalanced parenthesis",
                _ => "Malformed expression",
            };
            return self.err(msg);
        }
        Ok(())
    }

    fn expr(&mut self) -> Result<Expr> {
        self.bor()
    }

    fn bor(&mut self) -> Result<Expr> {
        let mut l = self.bxor()?;
        while self.eat_kw("or") {
            let r = self.bxor()?;
            l = Expr::Bin(BinOp::Or, l.into(), r.into());
        }
        Ok(l)
    }
    fn bxor(&mut self) -> Result<Expr> {
        let mut l = self.band()?;
        while self.eat_kw("xor") {
            let r = self.band()?;
            l = Expr::Bin(BinOp::Xor, l.into(), r.into());
        }
        Ok(l)
    }
    fn band(&mut self) -> Result<Expr> {
        let mut l = self.shift()?;
        while self.eat_kw("and") {
            let r = self.shift()?;
            l = Expr::Bin(BinOp::And, l.into(), r.into());
        }
        Ok(l)
    }
    fn shift(&mut self) -> Result<Expr> {
        let mut l = self.additive()?;
        loop {
            let op = if self.eat(&Tok::Shl) {
                BinOp::Shl
            } else if self.eat(&Tok::Shr) {
                BinOp::Shr
            } else {
                break;
            };
            let r = self.additive()?;
            l = Expr::Bin(op, l.into(), r.into());
        }
        Ok(l)
    }

    fn additive(&mut self) -> Result<Expr> {
        let mut l = self.mult()?;
        loop {
            let op = if self.eat(&Tok::Plus) {
                BinOp::Add
            } else if self.eat(&Tok::Minus) {
                BinOp::Sub
            } else {
                break;
            };
            let r = self.mult()?;
            // a ± b%  →  a ± a·b/100
            let r = match r {
                Expr::Percent(inner) => {
                    Expr::Bin(BinOp::Mul, l.clone().into(), Expr::Percent(inner).into())
                }
                r => r,
            };
            l = Expr::Bin(op, l.into(), r.into());
        }
        Ok(l)
    }

    fn starts_primary(&self) -> bool {
        match self.peek() {
            Some(Tok::Num { .. })
            | Some(Tok::LParen)
            | Some(Tok::Sqrt)
            | Some(Tok::Cbrt)
            | Some(Tok::Pi) => true,
            Some(Tok::Ident(w)) => !KEYWORDS.contains(&w.to_lowercase().as_str()),
            _ => false,
        }
    }

    fn mult(&mut self) -> Result<Expr> {
        let mut l = self.unary()?;
        loop {
            let op = if self.eat(&Tok::Mul) {
                BinOp::Mul
            } else if self.eat(&Tok::Div) {
                BinOp::Div
            } else if self.eat_kw("mod") {
                BinOp::Mod
            } else if self.starts_primary() {
                BinOp::Mul // implicit multiplication
            } else {
                break;
            };
            let r = self.unary()?;
            l = Expr::Bin(op, l.into(), r.into());
        }
        Ok(l)
    }

    fn unary(&mut self) -> Result<Expr> {
        if self.eat(&Tok::Minus) {
            return Ok(Expr::Neg(self.unary()?.into()));
        }
        if self.eat(&Tok::Plus) {
            return self.unary();
        }
        if self.eat_kw("not") {
            return Ok(Expr::Not(self.unary()?.into()));
        }
        self.power()
    }

    fn power(&mut self) -> Result<Expr> {
        let base = self.postfix()?;
        if self.eat(&Tok::Pow) {
            let exp = self.unary()?; // right associative, allows 2^-1
            return Ok(Expr::Bin(BinOp::Pow, base.into(), exp.into()));
        }
        Ok(base)
    }

    fn postfix(&mut self) -> Result<Expr> {
        let mut e = self.primary()?;
        loop {
            match self.peek() {
                Some(Tok::Bang) => {
                    self.i += 1;
                    e = Expr::Factorial(e.into());
                }
                Some(Tok::Percent) => {
                    self.i += 1;
                    e = Expr::Percent(e.into());
                }
                Some(Tok::Sup(s)) => {
                    let s = s.clone();
                    self.i += 1;
                    let exp = sup_expr(&s);
                    e = Expr::Bin(BinOp::Pow, e.into(), exp.into());
                }
                Some(Tok::Sqrt) if matches!(e, Expr::Num { .. }) => {
                    // n√x
                    self.i += 1;
                    let x = self.unary()?;
                    e = Expr::Root(e.into(), x.into());
                }
                _ => break,
            }
        }
        Ok(e)
    }

    fn primary(&mut self) -> Result<Expr> {
        let pos = self.pos();
        match self.bump() {
            Some(Tok::Num { digits, base }) => Ok(Expr::Num { digits, base }),
            Some(Tok::Pi) => Ok(Expr::Const(std::f64::consts::PI)),
            Some(Tok::LParen) => {
                let e = self.expr()?;
                if !self.eat(&Tok::RParen) {
                    // Auto-close at end of input, otherwise it's an error.
                    if self.peek().is_some() {
                        return self.err("Unbalanced parenthesis");
                    }
                }
                Ok(e)
            }
            Some(Tok::Pipe) => {
                let e = self.expr()?;
                if !self.eat(&Tok::Pipe) {
                    return self.err("Missing closing “|”");
                }
                Ok(Expr::Call {
                    name: "abs".into(),
                    inverse: false,
                    power: None,
                    args: vec![e],
                })
            }
            Some(Tok::Sqrt) => {
                let x = self.unary()?;
                Ok(Expr::Call {
                    name: "sqrt".into(),
                    inverse: false,
                    power: None,
                    args: vec![x],
                })
            }
            Some(Tok::Cbrt) => {
                let x = self.unary()?;
                Ok(Expr::Call {
                    name: "cbrt".into(),
                    inverse: false,
                    power: None,
                    args: vec![x],
                })
            }
            Some(Tok::Ident(name)) => {
                let lower = name.to_lowercase();
                if FUNCTIONS.contains(&lower.as_str()) {
                    self.call(lower)
                } else if KEYWORDS.contains(&lower.as_str()) {
                    Err(CalcError::Syntax {
                        pos,
                        msg: format!("Unexpected “{name}”"),
                    })
                } else {
                    Ok(match lower.as_str() {
                        "pi" => Expr::Const(std::f64::consts::PI),
                        "e" if name == "e" => Expr::Const(std::f64::consts::E),
                        "rand" => Expr::Rand,
                        _ => Expr::Var(name),
                    })
                }
            }
            Some(_) => Err(CalcError::Syntax {
                pos,
                msg: "Malformed expression".into(),
            }),
            None => Err(CalcError::Syntax {
                pos,
                msg: "Expression is incomplete".into(),
            }),
        }
    }

    fn call(&mut self, name: String) -> Result<Expr> {
        let mut inverse = false;
        let mut power = None;
        if let Some(Tok::Sup(s)) = self.peek() {
            let s = s.clone();
            self.i += 1;
            if s == "-1" {
                inverse = true;
            } else {
                power = Some(Box::new(sup_expr(&s)));
            }
        }
        let args = if self.eat(&Tok::LParen) {
            let mut args = vec![self.expr()?];
            while self.eat(&Tok::Sep) {
                args.push(self.expr()?);
            }
            if !self.eat(&Tok::RParen) && self.peek().is_some() {
                return self.err("Unbalanced parenthesis");
            }
            args
        } else {
            vec![self.power()?]
        };
        Ok(Expr::Call {
            name,
            inverse,
            power,
            args,
        })
    }
}

fn sup_expr(s: &str) -> Expr {
    match s.strip_prefix('-') {
        Some(rest) => Expr::Neg(Box::new(Expr::Num {
            digits: rest.into(),
            base: Some(10),
        })),
        None => Expr::Num {
            digits: s.into(),
            base: Some(10),
        },
    }
}

fn is_builtin_const(name: &str) -> bool {
    matches!(name, "pi" | "e" | "rand" | "ans")
}

// ---------------------------------------------------------------- evaluation

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Real,
    /// Programming mode: integer arithmetic with the given word size (bits)
    /// and default input base.
    Integer {
        bits: u32,
        base: u32,
    },
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Value {
    Real(f64),
    /// Raw bits masked to the word size.
    Int(u64),
}

impl Value {
    pub fn as_f64(self, bits: u32) -> f64 {
        match self {
            Value::Real(x) => x,
            Value::Int(v) => to_signed(v, bits) as f64,
        }
    }
}

pub fn mask(bits: u32) -> u64 {
    if bits >= 64 {
        u64::MAX
    } else {
        (1u64 << bits) - 1
    }
}

/// Interprets `v` as a two's-complement number of `bits` bits.
pub fn to_signed(v: u64, bits: u32) -> i64 {
    let v = v & mask(bits);
    if bits >= 64 {
        v as i64
    } else if v >> (bits - 1) & 1 == 1 {
        (v | !mask(bits)) as i64
    } else {
        v as i64
    }
}

/// Evaluation context: variables, last answer and settings.
#[derive(Debug, Clone)]
pub struct Context {
    pub vars: HashMap<String, f64>,
    pub ans: Value,
    pub angle: AngleUnit,
    pub mode: Mode,
}

impl Default for Context {
    fn default() -> Self {
        Context {
            vars: HashMap::new(),
            ans: Value::Real(0.0),
            angle: AngleUnit::Degrees,
            mode: Mode::Real,
        }
    }
}

pub fn parse(input: &str, ctx: &Context) -> Result<Statement> {
    let base = match ctx.mode {
        Mode::Integer { base, .. } => base,
        Mode::Real => 10,
    };
    let lexer = Lexer {
        chars: input.chars().collect(),
        i: 0,
        default_base: base,
        vars: &ctx.vars,
    };
    let toks = lexer.tokens()?;
    let end = input.chars().count();
    Parser { toks, i: 0, end }.statement()
}

impl Context {
    /// Evaluates `input`, updating `ans` (and variables on assignment).
    pub fn evaluate(&mut self, input: &str) -> Result<Value> {
        // Unit conversion has its own surface syntax.
        if let Mode::Real = self.mode {
            if let Some(v) = self.try_conversion(input)? {
                self.ans = Value::Real(v);
                return Ok(self.ans);
            }
        }
        let stmt = parse(input, self)?;
        let (name, expr) = match stmt {
            Statement::Assign(n, e) => (Some(n), e),
            Statement::Expr(e) => (None, e),
        };
        let v = match self.mode {
            Mode::Real => {
                let x = self.eval_real(&expr)?;
                if x.is_nan() {
                    return Err(CalcError::Domain("Result is undefined".into()));
                }
                if x.is_infinite() {
                    return Err(CalcError::Overflow);
                }
                // Snap tiny rounding noise (e.g. sin 180°) to zero.
                Value::Real(if x.abs() < 1e-15 { 0.0 } else { x })
            }
            Mode::Integer { bits, .. } => Value::Int(self.eval_int(&expr, bits)? & mask(bits)),
        };
        if let Some(n) = name {
            let bits = self.bits();
            self.vars.insert(n, v.as_f64(bits));
        }
        self.ans = v;
        Ok(v)
    }

    fn bits(&self) -> u32 {
        match self.mode {
            Mode::Integer { bits, .. } => bits,
            Mode::Real => 64,
        }
    }

    fn try_conversion(&mut self, input: &str) -> Result<Option<f64>> {
        let Some((lhs, to_unit)) = crate::units::split_conversion(input) else {
            return Ok(None);
        };
        let (expr_text, from_unit) = lhs;
        let value = if expr_text.trim().is_empty() {
            1.0
        } else {
            let stmt = parse(expr_text, self)?;
            match stmt {
                Statement::Expr(e) => self.eval_real(&e)?,
                Statement::Assign(..) => return Ok(None),
            }
        };
        crate::units::convert(value, from_unit, to_unit)
            .map(Some)
            .ok_or_else(|| CalcError::Domain(format!("Cannot convert {from_unit} to {to_unit}")))
    }

    fn lookup(&self, name: &str) -> Result<f64> {
        if name.eq_ignore_ascii_case("ans") {
            return Ok(self.ans.as_f64(self.bits()));
        }
        self.vars
            .get(name)
            .copied()
            .ok_or_else(|| CalcError::UnknownVariable(name.into()))
    }

    pub fn eval_real(&self, e: &Expr) -> Result<f64> {
        Ok(match e {
            Expr::Num { digits, base } => parse_real(digits, base.unwrap_or(10))?,
            Expr::Const(c) => *c,
            Expr::Rand => rand_f64(),
            Expr::Var(n) => self.lookup(n)?,
            Expr::Neg(x) => -self.eval_real(x)?,
            Expr::Not(_) => {
                return Err(CalcError::Domain(
                    "Boolean NOT is only defined for integers".into(),
                ))
            }
            Expr::Factorial(x) => factorial(self.eval_real(x)?)?,
            Expr::Percent(x) => self.eval_real(x)? / 100.0,
            Expr::Root(n, x) => {
                let n = self.eval_real(n)?;
                let x = self.eval_real(x)?;
                nth_root(x, n)?
            }
            Expr::Bin(op, a, b) => {
                let a = self.eval_real(a)?;
                let b = self.eval_real(b)?;
                match op {
                    BinOp::Add => a + b,
                    BinOp::Sub => a - b,
                    BinOp::Mul => a * b,
                    BinOp::Div => {
                        if b == 0.0 {
                            return Err(CalcError::DivisionByZero);
                        }
                        a / b
                    }
                    BinOp::Mod => {
                        if b == 0.0 {
                            return Err(CalcError::DivisionByZero);
                        }
                        a - b * (a / b).floor()
                    }
                    BinOp::Pow => {
                        if a == 0.0 && b < 0.0 {
                            return Err(CalcError::DivisionByZero);
                        }
                        if a < 0.0 && b.fract() != 0.0 {
                            // Odd roots of negatives: (-8)^(1/3) = -2.
                            let inv = 1.0 / b;
                            if (inv.round() - inv).abs() < 1e-9 && inv.round() as i64 % 2 != 0 {
                                return Ok(-(-a).powf(b));
                            }
                            return Err(CalcError::Domain(
                                "Negative numbers have no real fractional powers".into(),
                            ));
                        }
                        a.powf(b)
                    }
                    BinOp::And | BinOp::Or | BinOp::Xor | BinOp::Shl | BinOp::Shr => {
                        let ai = as_exact_int(a)?;
                        let bi = as_exact_int(b)?;
                        (match op {
                            BinOp::And => ai & bi,
                            BinOp::Or => ai | bi,
                            BinOp::Xor => ai ^ bi,
                            BinOp::Shl => ai.checked_shl(bi as u32).ok_or(CalcError::Overflow)?,
                            _ => ai.checked_shr(bi as u32).unwrap_or(0),
                        }) as f64
                    }
                }
            }
            Expr::Call {
                name,
                inverse,
                power,
                args,
            } => {
                let vals = args
                    .iter()
                    .map(|a| self.eval_real(a))
                    .collect::<Result<Vec<_>>>()?;
                let mut r = self.call_real(name, *inverse, &vals)?;
                if let Some(p) = power {
                    r = r.powf(self.eval_real(p)?);
                }
                r
            }
        })
    }

    fn call_real(&self, name: &str, inverse: bool, args: &[f64]) -> Result<f64> {
        if let Some(r) = financial::call(name, args) {
            return r;
        }
        let one = |n: usize| -> Result<f64> {
            if args.len() != n {
                Err(CalcError::Arity {
                    func: name.into(),
                    expected: n,
                })
            } else {
                Ok(args[0])
            }
        };
        let x = one(1)?;
        let name = if inverse {
            match name {
                "sin" => "asin",
                "cos" => "acos",
                "tan" => "atan",
                "sinh" => "asinh",
                "cosh" => "acosh",
                "tanh" => "atanh",
                "ln" => "exp",
                _ => return Err(CalcError::Domain(format!("{name} has no inverse"))),
            }
        } else {
            name
        };
        let a = self.angle;
        let domain = |ok: bool, msg: &str| {
            if ok {
                Ok(())
            } else {
                Err(CalcError::Domain(msg.into()))
            }
        };
        Ok(match name {
            "sin" => a.to_radians(x).sin(),
            "cos" => a.to_radians(x).cos(),
            "tan" => {
                let c = a.to_radians(x).cos();
                domain(
                    c.abs() > 1e-15,
                    "Tangent is undefined for angles that are multiples of π (180°) from π∕2 (90°)",
                )?;
                a.to_radians(x).tan()
            }
            "asin" => {
                domain(
                    (-1.0..=1.0).contains(&x),
                    "Inverse sine is undefined for values outside [-1, 1]",
                )?;
                a.radians_to_unit(x.asin())
            }
            "acos" => {
                domain(
                    (-1.0..=1.0).contains(&x),
                    "Inverse cosine is undefined for values outside [-1, 1]",
                )?;
                a.radians_to_unit(x.acos())
            }
            "atan" => a.radians_to_unit(x.atan()),
            "sinh" => x.sinh(),
            "cosh" => x.cosh(),
            "tanh" => x.tanh(),
            "asinh" => x.asinh(),
            "acosh" => {
                domain(
                    x >= 1.0,
                    "Inverse hyperbolic cosine is undefined for values less than one",
                )?;
                x.acosh()
            }
            "atanh" => {
                domain(
                    x > -1.0 && x < 1.0,
                    "Inverse hyperbolic tangent is undefined for values outside (-1, 1)",
                )?;
                x.atanh()
            }
            "ln" => {
                domain(x > 0.0, "Logarithm of zero or negative values is undefined")?;
                x.ln()
            }
            "log" => {
                domain(x > 0.0, "Logarithm of zero or negative values is undefined")?;
                x.log10()
            }
            "log2" => {
                domain(x > 0.0, "Logarithm of zero or negative values is undefined")?;
                x.log2()
            }
            "exp" => x.exp(),
            "sqrt" => nth_root(x, 2.0)?,
            "cbrt" => x.cbrt(),
            "abs" => x.abs(),
            "int" => x.trunc(),
            "frac" => x.fract(),
            "round" => x.round(),
            "floor" => x.floor(),
            "ceil" => x.ceil(),
            "sgn" => {
                if x == 0.0 {
                    0.0
                } else {
                    x.signum()
                }
            }
            "ones" | "twos" => {
                return Err(CalcError::Domain(format!(
                    "{name} is only available in programming mode"
                )))
            }
            _ => return Err(CalcError::UnknownFunction(name.into())),
        })
    }

    pub fn eval_int(&self, e: &Expr, bits: u32) -> Result<u64> {
        let m = mask(bits);
        let r = match e {
            Expr::Num { digits, base } => {
                let base = base.unwrap_or(match self.mode {
                    Mode::Integer { base, .. } => base,
                    Mode::Real => 10,
                });
                if digits.contains(['.', 'e', 'E']) && base == 10 {
                    return Err(CalcError::NotInteger);
                }
                u64::from_str_radix(digits, base).map_err(|_| CalcError::Overflow)?
            }
            Expr::Const(_) | Expr::Rand | Expr::Root(..) | Expr::Percent(_) => {
                return Err(CalcError::NotInteger)
            }
            Expr::Var(n) => {
                let v = self.lookup(n)?;
                if v.fract() != 0.0 {
                    return Err(CalcError::NotInteger);
                }
                v as i64 as u64
            }
            Expr::Neg(x) => self.eval_int(x, bits)?.wrapping_neg(),
            Expr::Not(x) => !self.eval_int(x, bits)?,
            Expr::Factorial(x) => {
                let n = to_signed(self.eval_int(x, bits)?, bits);
                if n < 0 {
                    return Err(CalcError::Domain(
                        "Factorial is only defined for non-negative integers".into(),
                    ));
                }
                let mut acc: u64 = 1;
                for i in 2..=n as u64 {
                    acc = acc.checked_mul(i).ok_or(CalcError::Overflow)?;
                }
                acc
            }
            Expr::Bin(op, a, b) => {
                let a = self.eval_int(a, bits)?;
                let b = self.eval_int(b, bits)?;
                let (sa, sb) = (to_signed(a, bits), to_signed(b, bits));
                match op {
                    BinOp::Add => a.wrapping_add(b),
                    BinOp::Sub => a.wrapping_sub(b),
                    BinOp::Mul => a.wrapping_mul(b),
                    BinOp::Div => {
                        if sb == 0 {
                            return Err(CalcError::DivisionByZero);
                        }
                        sa.wrapping_div(sb) as u64
                    }
                    BinOp::Mod => {
                        if sb == 0 {
                            return Err(CalcError::DivisionByZero);
                        }
                        sa.rem_euclid(sb) as u64
                    }
                    BinOp::Pow => {
                        if sb < 0 {
                            return Err(CalcError::NotInteger);
                        }
                        a.wrapping_pow(sb.min(u32::MAX as i64) as u32)
                    }
                    BinOp::And => a & b,
                    BinOp::Or => a | b,
                    BinOp::Xor => a ^ b,
                    BinOp::Shl => {
                        if b >= bits as u64 {
                            0
                        } else {
                            a << b
                        }
                    }
                    BinOp::Shr => {
                        if b >= bits as u64 {
                            0
                        } else {
                            (a & m) >> b
                        }
                    }
                }
            }
            Expr::Call { name, args, .. } => {
                if args.len() != 1 {
                    return Err(CalcError::Arity {
                        func: name.clone(),
                        expected: 1,
                    });
                }
                let x = self.eval_int(&args[0], bits)?;
                let sx = to_signed(x, bits);
                match name.as_str() {
                    "ones" => !x,
                    "twos" => (!x).wrapping_add(1),
                    "abs" => sx.unsigned_abs(),
                    "sgn" => sx.signum() as u64,
                    "int" | "round" | "floor" | "ceil" => x,
                    "frac" => 0,
                    "sqrt" => {
                        if sx < 0 {
                            return Err(CalcError::Domain(
                                "Square root of a negative number".into(),
                            ));
                        }
                        let r = (sx as f64).sqrt();
                        if r.fract() != 0.0 {
                            return Err(CalcError::NotInteger);
                        }
                        r as u64
                    }
                    _ => return Err(CalcError::NotInteger),
                }
            }
        };
        Ok(r & m)
    }
}

fn as_exact_int(x: f64) -> Result<i64> {
    if x.fract() != 0.0 || x.abs() > 9.0e15 {
        Err(CalcError::Domain(
            "Boolean operations are only defined for integers".into(),
        ))
    } else {
        Ok(x as i64)
    }
}

fn parse_real(digits: &str, base: u32) -> Result<f64> {
    if base == 10 {
        digits.parse::<f64>().map_err(|_| CalcError::Syntax {
            pos: 0,
            msg: "Malformed number".into(),
        })
    } else {
        let mut v = 0f64;
        for c in digits.chars() {
            let d = c.to_digit(base).ok_or(CalcError::Syntax {
                pos: 0,
                msg: format!("Digit “{c}” is not valid in base {base}"),
            })?;
            v = v * base as f64 + d as f64;
        }
        Ok(v)
    }
}

fn nth_root(x: f64, n: f64) -> Result<f64> {
    if n == 0.0 {
        return Err(CalcError::DivisionByZero);
    }
    if x < 0.0 {
        if n.fract() == 0.0 && (n as i64) % 2 != 0 {
            return Ok(-(-x).powf(1.0 / n));
        }
        return Err(CalcError::Domain(
            "Even roots of negative numbers are undefined".into(),
        ));
    }
    Ok(x.powf(1.0 / n))
}

/// n! for non-negative integers; Γ(x+1) for other non-negative reals.
pub fn factorial(x: f64) -> Result<f64> {
    if x < 0.0 {
        return Err(CalcError::Domain(
            "Factorial is only defined for non-negative numbers".into(),
        ));
    }
    if x.fract() == 0.0 {
        if x > 170.0 {
            return Err(CalcError::Overflow);
        }
        return Ok((2..=x as u64).fold(1.0, |acc, i| acc * i as f64));
    }
    Ok(gamma(x + 1.0))
}

/// Lanczos approximation of the gamma function.
fn gamma(x: f64) -> f64 {
    const G: f64 = 7.0;
    const C: [f64; 9] = [
        0.999_999_999_999_809_9,
        676.520_368_121_885_1,
        -1_259.139_216_722_402_8,
        771.323_428_777_653_1,
        -176.615_029_162_140_6,
        12.507_343_278_686_905,
        -0.138_571_095_265_720_12,
        9.984_369_578_019_572e-6,
        1.505_632_735_149_311_6e-7,
    ];
    if x < 0.5 {
        std::f64::consts::PI / ((std::f64::consts::PI * x).sin() * gamma(1.0 - x))
    } else {
        let x = x - 1.0;
        let mut a = C[0];
        let t = x + G + 0.5;
        for (i, c) in C.iter().enumerate().skip(1) {
            a += c / (x + i as f64);
        }
        (2.0 * std::f64::consts::PI).sqrt() * t.powf(x + 0.5) * (-t).exp() * a
    }
}

fn rand_f64() -> f64 {
    use std::cell::Cell;
    use std::time::{SystemTime, UNIX_EPOCH};
    thread_local! {
        static STATE: Cell<u64> = Cell::new(
            SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_nanos() as u64).unwrap_or(0x9E37_79B9) | 1,
        );
    }
    STATE.with(|s| {
        // xorshift64*
        let mut x = s.get();
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        s.set(x);
        (x.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 11) as f64 / (1u64 << 53) as f64
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn eval(s: &str) -> f64 {
        let mut ctx = Context::default();
        match ctx.evaluate(s) {
            Ok(Value::Real(x)) => x,
            other => panic!("{s}: {other:?}"),
        }
    }

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() <= 1e-9 * b.abs().max(1.0)
    }

    #[test]
    fn precedence() {
        assert_eq!(eval("1+2×3"), 7.0);
        assert_eq!(eval("(1+2)*3"), 9.0);
        assert_eq!(eval("2^3^2"), 512.0);
        assert_eq!(eval("-2^2"), -4.0);
        assert_eq!(eval("2^-1"), 0.5);
        assert_eq!(eval("10−4÷2"), 8.0);
        assert_eq!(eval("7 mod 3"), 1.0);
        assert_eq!(eval("-7 mod 3"), 2.0);
    }

    #[test]
    fn implicit_and_superscripts() {
        assert!(close(eval("2π"), 2.0 * std::f64::consts::PI));
        assert_eq!(eval("3(4+1)"), 15.0);
        assert_eq!(eval("(1+2)(3+4)"), 21.0);
        assert_eq!(eval("3²"), 9.0);
        assert_eq!(eval("2¹⁰"), 1024.0);
        assert_eq!(eval("4⁻¹"), 0.25);
        assert_eq!(eval("1.5×10³"), 1500.0);
        assert_eq!(eval("1.5e3"), 1500.0);
    }

    #[test]
    fn roots_and_factorials() {
        assert_eq!(eval("√16"), 4.0);
        assert_eq!(eval("∛27"), 3.0);
        assert_eq!(eval("3√8"), 2.0);
        assert_eq!(eval("∛-8"), -2.0);
        assert_eq!(eval("5!"), 120.0);
        assert!(close(eval("0.5!"), std::f64::consts::PI.sqrt() / 2.0));
        assert_eq!(eval("|-3|"), 3.0);
    }

    #[test]
    fn percent() {
        assert_eq!(eval("50%"), 0.5);
        assert_eq!(eval("200+10%"), 220.0);
        assert_eq!(eval("200−10%"), 180.0);
        assert_eq!(eval("200×10%"), 20.0);
    }

    #[test]
    fn functions_and_angles() {
        assert!(close(eval("sin 30"), 0.5));
        assert!(close(eval("cos(60)"), 0.5));
        assert_eq!(eval("sin 180"), 0.0);
        assert!(close(eval("sin⁻¹ 0.5"), 30.0));
        assert!(close(eval("sin²30"), 0.25));
        assert_eq!(eval("log 1000"), 3.0);
        assert!(close(eval("ln e"), 1.0));
        let mut ctx = Context {
            angle: AngleUnit::Radians,
            ..Default::default()
        };
        assert_eq!(ctx.evaluate("cos 0"), Ok(Value::Real(1.0)));
        let mut ctx = Context {
            angle: AngleUnit::Gradians,
            ..Default::default()
        };
        assert!(close(ctx.evaluate("sin 100").unwrap().as_f64(64), 1.0));
    }

    #[test]
    fn variables_and_ans() {
        let mut ctx = Context::default();
        ctx.evaluate("x = 4").unwrap();
        assert_eq!(ctx.evaluate("x²+1"), Ok(Value::Real(17.0)));
        assert_eq!(ctx.evaluate("ans×2"), Ok(Value::Real(34.0)));
        assert_eq!(ctx.evaluate("2x"), Ok(Value::Real(8.0)));
        assert!(matches!(
            ctx.evaluate("y+1"),
            Err(CalcError::UnknownVariable(_))
        ));
        assert!(matches!(
            ctx.evaluate("sin = 3"),
            Err(CalcError::Syntax { .. })
        ));
    }

    #[test]
    fn errors() {
        let mut ctx = Context::default();
        assert_eq!(ctx.evaluate("1/0"), Err(CalcError::DivisionByZero));
        assert!(matches!(ctx.evaluate("ln 0"), Err(CalcError::Domain(_))));
        assert!(matches!(ctx.evaluate("1+"), Err(CalcError::Syntax { .. })));
        assert!(ctx.evaluate("(1+2").is_ok(), "trailing paren auto-closes");
        assert!(matches!(
            ctx.evaluate("1+2)"),
            Err(CalcError::Syntax { .. })
        ));
        assert!(matches!(
            ctx.evaluate("1 $ 2"),
            Err(CalcError::Syntax { pos: 2, .. })
        ));
        assert!(matches!(ctx.evaluate("tan 90"), Err(CalcError::Domain(_))));
        assert_eq!(ctx.evaluate(""), Err(CalcError::Empty));
    }

    #[test]
    fn thousands_and_args() {
        assert_eq!(eval("1,234+1"), 1235.0);
        assert!(close(eval("fv(100, 0.05, 10)"), 1_257.789_253_554_883));
        assert!(close(eval("pmt(1000;5%;12)"), 112.825_410_021_13));
    }

    #[test]
    fn programming_mode() {
        let mut ctx = Context {
            mode: Mode::Integer { bits: 8, base: 16 },
            ..Default::default()
        };
        assert_eq!(ctx.evaluate("FF+1"), Ok(Value::Int(0)));
        assert_eq!(ctx.evaluate("A and 3"), Ok(Value::Int(2)));
        assert_eq!(ctx.evaluate("1 << 4"), Ok(Value::Int(0x10)));
        assert_eq!(ctx.evaluate("not 0"), Ok(Value::Int(0xFF)));
        assert_eq!(ctx.evaluate("twos 1"), Ok(Value::Int(0xFF)));
        assert_eq!(ctx.evaluate("0b101 xor 0b011"), Ok(Value::Int(6)));
        assert_eq!(
            ctx.evaluate("e"),
            Ok(Value::Int(14)),
            "hex digit, not Euler's number"
        );
        let mut ctx = Context {
            mode: Mode::Integer { bits: 32, base: 10 },
            ..Default::default()
        };
        assert_eq!(ctx.evaluate("7/2"), Ok(Value::Int(3)));
        assert_eq!(ctx.evaluate("0-1"), Ok(Value::Int(0xFFFF_FFFF)));
        assert_eq!(ctx.evaluate("1.5"), Err(CalcError::NotInteger));
        assert_eq!(ctx.evaluate("FF₁₆"), Ok(Value::Int(255)));
        assert_eq!(to_signed(0xFF, 8), -1);
    }

    #[test]
    fn conversions() {
        assert!(close(eval("12 in in cm"), 30.48));
        assert!(close(eval("100 degF in degC"), 37.777_777_777_777_78));
        assert!(close(eval("1 km in m"), 1000.0));
        assert!(close(eval("2×3 kg to g"), 6000.0));
    }
}
