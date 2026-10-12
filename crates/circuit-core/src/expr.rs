//! The expression language (LLD §17 D4): behavioral laws, value rules, derived params and the
//! expressions inside imported SPICE models share one hand-written parser, a dimensional
//! checker, an evaluator and a SPICE emitter. SPICE text is only ever written from the tree,
//! never copied from what a user or a model typed.
//!
//! Two dialects share the tree. [`Dialect::Law`] is what people write: case-sensitive SI
//! prefixes as in [`crate::units`] (`m` milli, `M` mega), optional unit suffixes (`1.5V`,
//! `10kΩ`) and the allow-listed functions of D4. [`Dialect::Spice`] reads vendor models: SPICE's
//! case-insensitive scale factors (`1M` is milli, `1MEG` mega, trailing letters ignored) and
//! the functions ngspice's B-sources know.
//!
//! Dimensions: every term carries volts, amps and seconds to some power, so `V(in) + I(out)` is
//! an error with a plain message. A plain number (`1.5`, `pi`) takes the dimension its partner
//! in a sum or a comparison needs, so `V(vcc) - 1.5` reads as 1.5 V.
//!
//! Determinism: `+ − × ÷ √` are exact in IEEE 754 on every runtime; integer powers are computed
//! (and emitted) as repeated products, never through `pow`; results of transcendental functions
//! are flagged [`Eval::inexact`], so a value rule rounds them before they choose a part value.

use std::collections::BTreeMap;
use std::fmt;

use crate::units::{Unit, spice_number};

/// Longest text the parser accepts for one expression.
pub const MAX_LEN: usize = 65_536;
/// Deepest nesting of parentheses and calls.
pub const MAX_DEPTH: usize = 64;
/// Integer powers up to this exponent are repeated products.
const MAX_INT_POW: f64 = 64.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dialect {
    Law,
    Spice,
}

// ---------------------------------------------------------------- dimensions

/// Exponents of volts, amps and seconds: ohms are V·A⁻¹, farads A·s·V⁻¹, hertz s⁻¹.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Dim {
    pub volt: i8,
    pub amp: i8,
    pub sec: i8,
}

impl Dim {
    pub const NONE: Dim = Dim { volt: 0, amp: 0, sec: 0 };

    pub fn of(u: Unit) -> Dim {
        let d = |volt, amp, sec| Dim { volt, amp, sec };
        match u {
            Unit::Ohm => d(1, -1, 0),
            Unit::Farad => d(-1, 1, 1),
            Unit::Henry => d(1, -1, 1),
            Unit::Volt => d(1, 0, 0),
            Unit::Ampere => d(0, 1, 0),
            Unit::Hertz => d(0, 0, -1),
            Unit::Second => d(0, 0, 1),
            Unit::Watt => d(1, 1, 0),
            Unit::Unitless | Unit::Decibel => Dim::NONE,
        }
    }

    /// The unit with exactly this dimension, if there is one.
    pub fn unit(self) -> Option<Unit> {
        Unit::ALL.into_iter().find(|u| *u != Unit::Decibel && Dim::of(*u) == self)
    }

    fn mul(self, o: Dim) -> Option<Dim> {
        Some(Dim {
            volt: self.volt.checked_add(o.volt)?,
            amp: self.amp.checked_add(o.amp)?,
            sec: self.sec.checked_add(o.sec)?,
        })
    }

    fn inv(self) -> Dim {
        Dim { volt: -self.volt, amp: -self.amp, sec: -self.sec }
    }

    fn pow(self, n: i8) -> Option<Dim> {
        Some(Dim { volt: self.volt.checked_mul(n)?, amp: self.amp.checked_mul(n)?, sec: self.sec.checked_mul(n)? })
    }

    fn half(self) -> Option<Dim> {
        (self.volt % 2 == 0 && self.amp % 2 == 0 && self.sec % 2 == 0).then_some(Dim {
            volt: self.volt / 2,
            amp: self.amp / 2,
            sec: self.sec / 2,
        })
    }

    /// In words: "volts", "ohms", or the base units ("V²·s").
    pub fn describe(self) -> String {
        match self.unit() {
            Some(Unit::Unitless) => "a plain number".into(),
            Some(Unit::Ohm) => "ohms".into(),
            Some(Unit::Farad) => "farads".into(),
            Some(Unit::Henry) => "henries".into(),
            Some(Unit::Volt) => "volts".into(),
            Some(Unit::Ampere) => "amps".into(),
            Some(Unit::Hertz) => "hertz".into(),
            Some(Unit::Second) => "seconds".into(),
            Some(Unit::Watt) => "watts".into(),
            _ => {
                let term = |sym: &str, e: i8| if e == 1 { sym.to_string() } else { format!("{sym}^{e}") };
                let all = [("V", self.volt), ("A", self.amp), ("s", self.sec)];
                let up: Vec<String> = all.iter().filter(|(_, e)| *e > 0).map(|(s, e)| term(s, *e)).collect();
                let down: Vec<String> = all.iter().filter(|(_, e)| *e < 0).map(|(s, e)| term(s, -*e)).collect();
                let up = if up.is_empty() { "1".to_string() } else { up.join("·") };
                if down.is_empty() { up } else { format!("{up}/{}", down.join("·")) }
            }
        }
    }
}

/// The type of an expression: a dimension, or `Free` for plain numbers, which take whatever
/// dimension the expression around them needs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ty {
    Free,
    Dim(Dim),
}

impl Ty {
    pub fn unitless() -> Ty {
        Ty::Dim(Dim::NONE)
    }

    /// The dimension, plain numbers counting as unitless.
    pub fn dim(self) -> Dim {
        match self {
            Ty::Free => Dim::NONE,
            Ty::Dim(d) => d,
        }
    }

    /// Whether a value of this type can be stored in `unit`.
    pub fn fits(self, unit: Unit) -> bool {
        match self {
            Ty::Free => true,
            Ty::Dim(d) => d == Dim::of(unit),
        }
    }
}

// ---------------------------------------------------------------- tree

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Pow,
    Lt,
    Le,
    Gt,
    Ge,
    Eq,
    Ne,
    And,
    Or,
}

impl BinOp {
    fn symbol(self) -> &'static str {
        match self {
            BinOp::Add => "+",
            BinOp::Sub => "-",
            BinOp::Mul => "*",
            BinOp::Div => "/",
            BinOp::Pow => "^",
            BinOp::Lt => "<",
            BinOp::Le => "<=",
            BinOp::Gt => ">",
            BinOp::Ge => ">=",
            BinOp::Eq => "==",
            BinOp::Ne => "!=",
            BinOp::And => "&&",
            BinOp::Or => "||",
        }
    }

    fn precedence(self) -> u8 {
        match self {
            BinOp::Or => 1,
            BinOp::And => 2,
            BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge | BinOp::Eq | BinOp::Ne => 3,
            BinOp::Add | BinOp::Sub => 4,
            BinOp::Mul | BinOp::Div => 5,
            BinOp::Pow => 7,
        }
    }

    fn is_comparison(self) -> bool {
        matches!(self, BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge | BinOp::Eq | BinOp::Ne)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnOp {
    Neg,
    Not,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Expr {
    /// A number in SI units, with the unit it was written in (`1.5V`), if any.
    Num(f64, Option<Unit>),
    /// A param, target, rule step or `.param` by name.
    Var(String),
    /// `V(a)` or `V(a, b)`: a pin's voltage, or the difference of two.
    Volt(String, Option<String>),
    /// `I(pin)`: current into a pin (laws); `i(vname)`: a voltage source's current (SPICE).
    Amp(String),
    Time,
    Pi,
    /// ngspice's circuit temperature, read only by imported models.
    Temper,
    Unary(UnOp, Box<Expr>),
    Bin(BinOp, Box<Expr>, Box<Expr>),
    /// A built-in function (lowercase name) or a user function before [`inline`].
    Call(String, Vec<Expr>),
    /// `if(c, a, b)` or `c ? a : b`.
    Cond(Box<Expr>, Box<Expr>, Box<Expr>),
    /// `[a, b, c]`: only the coefficient lists of `tf`.
    List(Vec<Expr>),
    /// A variable bound to a numbered slot ([`bind`]), read through [`Values::slot`].
    Slot(usize),
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("{message}")]
pub struct ExprError {
    pub message: String,
    /// Character offset in the text, when the problem has a place.
    pub at: Option<usize>,
}

pub fn error(message: impl Into<String>) -> ExprError {
    ExprError { message: message.into(), at: None }
}

fn error_at(at: usize, message: impl Into<String>) -> ExprError {
    ExprError { message: message.into(), at: Some(at) }
}

// ---------------------------------------------------------------- built-in functions

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Shape {
    /// Unitless in, unitless out (`exp`, `sin`).
    Plain,
    /// Same dimension in and out (`abs`, `floor`).
    Same,
    /// Any argument, unitless result (`sgn`, `u`).
    Sign,
    Sqrt,
    /// All arguments one dimension, result that dimension (`min`, `max`, `clamp`, `limit`).
    Unify,
    Pow,
    If,
    Table,
    /// `nearest(E24, x[, lo, hi])` and `search(E12, lo, hi[, min, max])`: value rules only.
    Series,
    Tf,
}

/// Allow-listed functions: name, argument count range, shape. Anything else is refused.
fn builtin(name: &str, dialect: Dialect) -> Option<(usize, usize, Shape)> {
    let common = match name {
        "exp" | "ln" | "log10" | "tanh" | "sin" | "cos" => (1, 1, Shape::Plain),
        "sqrt" => (1, 1, Shape::Sqrt),
        "abs" => (1, 1, Shape::Same),
        "min" | "max" => (2, 2, Shape::Unify),
        "pow" => (2, 2, Shape::Pow),
        "if" => (3, 3, Shape::If),
        _ => (0, 0, Shape::Plain),
    };
    if common.1 > 0 {
        return Some(common);
    }
    match dialect {
        Dialect::Law => match name {
            "clamp" => Some((3, 3, Shape::Unify)),
            "table" => Some((5, 2 * 64 + 1, Shape::Table)),
            "nearest" => Some((2, 4, Shape::Series)),
            "search" => Some((3, 5, Shape::Series)),
            "tf" => Some((3, 3, Shape::Tf)),
            _ => None,
        },
        Dialect::Spice => match name {
            "log" | "tan" | "atan" | "asin" | "acos" | "sinh" | "cosh" => Some((1, 1, Shape::Plain)),
            "floor" | "ceil" | "uramp" => Some((1, 1, Shape::Same)),
            "sgn" | "u" => Some((1, 1, Shape::Sign)),
            "limit" => Some((3, 3, Shape::Unify)),
            "pwr" | "pwrs" => Some((2, 2, Shape::Pow)),
            _ => None,
        },
    }
}

/// Functions whose results are rounded differently by different libm builds.
fn transcendental(name: &str) -> bool {
    matches!(
        name,
        "exp" | "ln" | "log" | "log10" | "tanh" | "sin" | "cos" | "tan" | "atan" | "asin" | "acos" | "sinh" | "cosh"
    )
}

// ---------------------------------------------------------------- lexer

#[derive(Clone, Debug, PartialEq)]
enum Tok {
    Num(f64, Option<Unit>),
    Ident(String),
    /// A node or source name inside `V(...)` / `I(...)`.
    Name(String),
    Sym(&'static str),
    End,
}

const SYMBOLS: [&str; 22] = [
    "**", "<=", ">=", "==", "!=", "&&", "||", "+", "-", "*", "/", "^", "(", ")", "[", "]", ",", "<", ">", "?", ":", "!",
];

fn lex(text: &str, dialect: Dialect) -> Result<Vec<(Tok, usize)>, ExprError> {
    if text.len() > MAX_LEN {
        return Err(error(format!("an expression is at most {MAX_LEN} characters")));
    }
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let (at, c) = chars[i];
        if c.is_whitespace() {
            i += 1;
            continue;
        }
        // Unicode operators people (and models) write.
        let uni = match c {
            '−' => Some("-"),
            '×' | '·' => Some("*"),
            '÷' => Some("/"),
            _ => None,
        };
        if let Some(s) = uni {
            out.push((Tok::Sym(s), at));
            i += 1;
            continue;
        }
        if c.is_ascii_digit() || (c == '.' && chars.get(i + 1).is_some_and(|(_, d)| d.is_ascii_digit())) {
            let (tok, next) = lex_number(&chars, i, dialect)?;
            out.push((tok, at));
            i = next;
            continue;
        }
        if c.is_alphabetic() || c == '_' {
            let start = i;
            while i < chars.len() {
                let ch = chars[i].1;
                let dotted = ch == '.'
                    && dialect == Dialect::Law
                    && chars.get(i + 1).is_some_and(|(_, d)| d.is_ascii_alphabetic());
                if ch.is_alphanumeric() || ch == '_' || dotted {
                    i += 1;
                } else {
                    break;
                }
            }
            let word: String = chars[start..i].iter().map(|(_, ch)| *ch).collect();
            let probe = word.to_ascii_lowercase();
            let mut j = i;
            while j < chars.len() && chars[j].1.is_whitespace() {
                j += 1;
            }
            let call = chars.get(j).is_some_and(|(_, ch)| *ch == '(');
            out.push((Tok::Ident(word.clone()), at));
            if call && (probe == "v" || probe == "i") {
                // Node names: anything up to `,` or `)`.
                out.push((Tok::Sym("("), chars[j].0));
                i = j + 1;
                loop {
                    let s = i;
                    while i < chars.len() && !matches!(chars[i].1, ',' | ')' | '(') {
                        i += 1;
                    }
                    let name: String = chars[s..i].iter().map(|(_, ch)| *ch).collect::<String>().trim().to_string();
                    let pos = chars.get(s).map_or(text.len(), |(p, _)| *p);
                    if name.is_empty() {
                        return Err(error_at(pos, format!("{word}( ) needs a name")));
                    }
                    if !name.chars().all(|ch| ch.is_alphanumeric() || "_$#.:-+".contains(ch)) {
                        return Err(error_at(pos, format!("\"{name}\" is not a node name")));
                    }
                    out.push((Tok::Name(name), pos));
                    match chars.get(i) {
                        Some((p, ',')) => {
                            out.push((Tok::Sym(","), *p));
                            i += 1;
                        }
                        Some((p, ')')) => {
                            out.push((Tok::Sym(")"), *p));
                            i += 1;
                            break;
                        }
                        _ => return Err(error_at(at, format!("{word}( is not closed"))),
                    }
                }
            }
            continue;
        }
        let rest = &text[at..];
        match SYMBOLS.iter().find(|s| rest.starts_with(**s)) {
            Some(s) => {
                out.push((Tok::Sym(s), at));
                i += s.chars().count();
            }
            None => return Err(error_at(at, format!("unexpected \"{c}\""))),
        }
    }
    out.push((Tok::End, text.len()));
    Ok(out)
}

/// Law-dialect suffixes after a number: an SI prefix, then optionally a unit.
const LAW_PREFIXES: [(&str, i32); 16] = [
    ("meg", 6),
    ("Meg", 6),
    ("MEG", 6),
    ("f", -15),
    ("p", -12),
    ("n", -9),
    ("u", -6),
    ("µ", -6),
    ("μ", -6),
    ("m", -3),
    ("k", 3),
    ("K", 3),
    ("M", 6),
    ("G", 9),
    ("T", 12),
    ("", 0),
];
const LAW_UNITS: [(&str, Unit); 13] = [
    ("ohms", Unit::Ohm),
    ("ohm", Unit::Ohm),
    ("Ohm", Unit::Ohm),
    ("Ω", Unit::Ohm),
    ("\u{2126}", Unit::Ohm),
    ("Hz", Unit::Hertz),
    ("dB", Unit::Decibel),
    ("V", Unit::Volt),
    ("A", Unit::Ampere),
    ("s", Unit::Second),
    ("F", Unit::Farad),
    ("H", Unit::Henry),
    ("W", Unit::Watt),
];

fn lex_number(chars: &[(usize, char)], start: usize, dialect: Dialect) -> Result<(Tok, usize), ExprError> {
    let at = chars[start].0;
    let mut i = start;
    let digits = |i: &mut usize| {
        let s = *i;
        while *i < chars.len() && chars[*i].1.is_ascii_digit() {
            *i += 1;
        }
        chars[s..*i].iter().map(|(_, c)| *c).collect::<String>()
    };
    let int = digits(&mut i);
    let mut frac = String::new();
    let mut point = false;
    if i < chars.len() && chars[i].1 == '.' {
        point = true;
        i += 1;
        frac = digits(&mut i);
    }
    let mut exp: i32 = 0;
    let mut has_exp = false;
    if i < chars.len() && matches!(chars[i].1, 'e' | 'E') {
        let mut j = i + 1;
        let neg = j < chars.len() && chars[j].1 == '-';
        if j < chars.len() && matches!(chars[j].1, '+' | '-') {
            j += 1;
        }
        if j < chars.len() && chars[j].1.is_ascii_digit() {
            let mut k = j;
            let e = digits(&mut k);
            let v: i32 = e.parse().map_err(|_| error_at(at, "exponent too large"))?;
            exp = if neg { -v } else { v };
            has_exp = true;
            i = k;
        }
    }
    let ls = i;
    while i < chars.len() && (chars[i].1.is_alphabetic()) {
        i += 1;
    }
    let letters: String = chars[ls..i].iter().map(|(_, c)| *c).collect();
    let decimal = |int: &str, frac: &str, e: i32| -> Result<f64, ExprError> {
        let int = if int.is_empty() { "0" } else { int };
        let frac = if frac.is_empty() { "0" } else { frac };
        let v: f64 = format!("{int}.{frac}e{e}").parse().map_err(|_| error_at(at, "not a number"))?;
        if v.is_finite() { Ok(v) } else { Err(error_at(at, "number out of range")) }
    };
    match dialect {
        Dialect::Spice => {
            let lower = letters.to_ascii_lowercase();
            let scale = if lower.starts_with("meg") {
                6
            } else if lower.starts_with("mil") {
                // 25.4e-6: exact enough as a product of decimals.
                let v = decimal(&int, &frac, exp)? * 25.4e-6;
                return Ok((Tok::Num(v, None), i));
            } else {
                match lower.chars().next() {
                    Some('t') => 12,
                    Some('g') => 9,
                    Some('k') => 3,
                    Some('m') => -3,
                    Some('u') => -6,
                    Some('n') => -9,
                    Some('p') => -12,
                    Some('f') => -15,
                    Some('a') => -18,
                    _ => 0,
                }
            };
            let total = exp.checked_add(scale).ok_or_else(|| error_at(at, "number out of range"))?;
            Ok((Tok::Num(decimal(&int, &frac, total)?, None), i))
        }
        Dialect::Law => {
            // RKM (`4k7`, `4R7`): digits, a prefix letter or R, digits.
            if !point && !has_exp && !letters.is_empty() && i < chars.len() && chars[i].1.is_ascii_digit() {
                let tail = digits(&mut i);
                let pexp = if letters == "R" || letters == "r" {
                    0
                } else {
                    LAW_PREFIXES
                        .iter()
                        .find(|(p, _)| *p == letters && !p.is_empty() && !p.eq_ignore_ascii_case("meg"))
                        .map(|(_, e)| *e)
                        .ok_or_else(|| error_at(at, format!("cannot read \"{letters}\" in a number")))?
                };
                return Ok((Tok::Num(decimal(&int, &tail, pexp)?, None), i));
            }
            for (p, pexp) in LAW_PREFIXES {
                let Some(rest) = letters.strip_prefix(p) else { continue };
                let unit = if rest.is_empty() {
                    None
                } else {
                    match LAW_UNITS.iter().find(|(u, _)| *u == rest) {
                        Some((_, u)) => Some(*u),
                        None => continue,
                    }
                };
                let total = exp.checked_add(pexp).ok_or_else(|| error_at(at, "number out of range"))?;
                return Ok((Tok::Num(decimal(&int, &frac, total)?, unit), i));
            }
            Err(error_at(at, format!("cannot read \"{letters}\" after a number (write 2*pi, not 2pi)")))
        }
    }
}

// ---------------------------------------------------------------- parser

struct Parser {
    toks: Vec<(Tok, usize)>,
    pos: usize,
    depth: usize,
    dialect: Dialect,
}

pub fn parse(text: &str, dialect: Dialect) -> Result<Expr, ExprError> {
    let mut p = Parser { toks: lex(text, dialect)?, pos: 0, depth: 0, dialect };
    let e = p.expr()?;
    match p.peek() {
        Tok::End => Ok(e),
        t => Err(error_at(p.at(), format!("unexpected {} after the expression", describe(t)))),
    }
}

fn describe(t: &Tok) -> String {
    match t {
        Tok::Num(..) => "a number".into(),
        Tok::Ident(s) | Tok::Name(s) => format!("\"{s}\""),
        Tok::Sym(s) => format!("\"{s}\""),
        Tok::End => "the end".into(),
    }
}

impl Parser {
    fn peek(&self) -> &Tok {
        &self.toks[self.pos].0
    }
    fn at(&self) -> usize {
        self.toks[self.pos].1
    }
    fn bump(&mut self) -> Tok {
        let t = self.toks[self.pos].0.clone();
        if self.pos + 1 < self.toks.len() {
            self.pos += 1;
        }
        t
    }
    fn eat(&mut self, s: &str) -> bool {
        if matches!(self.peek(), Tok::Sym(x) if *x == s) {
            self.bump();
            true
        } else {
            false
        }
    }
    fn expect(&mut self, s: &str) -> Result<(), ExprError> {
        if self.eat(s) {
            Ok(())
        } else {
            Err(error_at(self.at(), format!("expected \"{s}\", found {}", describe(self.peek()))))
        }
    }
    fn enter(&mut self) -> Result<(), ExprError> {
        self.depth += 1;
        if self.depth > MAX_DEPTH {
            return Err(error_at(self.at(), format!("expressions nest at most {MAX_DEPTH} deep")));
        }
        Ok(())
    }

    fn expr(&mut self) -> Result<Expr, ExprError> {
        self.enter()?;
        let c = self.binary(1)?;
        let out = if self.eat("?") {
            let a = self.expr()?;
            self.expect(":")?;
            let b = self.expr()?;
            Expr::Cond(Box::new(c), Box::new(a), Box::new(b))
        } else {
            c
        };
        self.depth -= 1;
        Ok(out)
    }

    fn binop(&self) -> Option<BinOp> {
        let Tok::Sym(s) = self.peek() else { return None };
        Some(match *s {
            "+" => BinOp::Add,
            "-" => BinOp::Sub,
            "*" => BinOp::Mul,
            "/" => BinOp::Div,
            "<" => BinOp::Lt,
            "<=" => BinOp::Le,
            ">" => BinOp::Gt,
            ">=" => BinOp::Ge,
            "==" => BinOp::Eq,
            "!=" => BinOp::Ne,
            "&&" => BinOp::And,
            "||" => BinOp::Or,
            _ => return None,
        })
    }

    /// Left-associative binary operators of at least `min` precedence; comparisons do not chain.
    fn binary(&mut self, min: u8) -> Result<Expr, ExprError> {
        let mut lhs = self.unary()?;
        while let Some(op) = self.binop() {
            let prec = op.precedence();
            if prec < min {
                break;
            }
            self.bump();
            let rhs = self.binary(prec + 1)?;
            if op.is_comparison() && self.binop().is_some_and(|o| o.is_comparison()) {
                return Err(error_at(self.at(), "comparisons do not chain: write (a < b) && (b < c)"));
            }
            lhs = Expr::Bin(op, Box::new(lhs), Box::new(rhs));
        }
        Ok(lhs)
    }

    fn unary(&mut self) -> Result<Expr, ExprError> {
        self.enter()?;
        let out = if self.eat("-") {
            Expr::Unary(UnOp::Neg, Box::new(self.unary()?))
        } else if self.eat("+") {
            self.unary()?
        } else if self.eat("!") {
            Expr::Unary(UnOp::Not, Box::new(self.unary()?))
        } else {
            let base = self.atom()?;
            if self.eat("^") || self.eat("**") {
                Expr::Bin(BinOp::Pow, Box::new(base), Box::new(self.unary()?))
            } else {
                base
            }
        };
        self.depth -= 1;
        Ok(out)
    }

    fn atom(&mut self) -> Result<Expr, ExprError> {
        let at = self.at();
        match self.bump() {
            Tok::Num(v, u) => Ok(Expr::Num(v, u)),
            Tok::Sym("(") => {
                let e = self.expr()?;
                self.expect(")")?;
                Ok(e)
            }
            Tok::Sym("[") => {
                self.enter()?;
                let mut items = Vec::new();
                if !self.eat("]") {
                    loop {
                        items.push(self.expr()?);
                        if self.eat("]") {
                            break;
                        }
                        self.expect(",")?;
                    }
                }
                self.depth -= 1;
                Ok(Expr::List(items))
            }
            Tok::Ident(word) => self.ident(word, at),
            t => Err(error_at(at, format!("expected a value, found {}", describe(&t)))),
        }
    }

    fn ident(&mut self, word: String, at: usize) -> Result<Expr, ExprError> {
        let lower = word.to_ascii_lowercase();
        let spice = self.dialect == Dialect::Spice;
        let call = matches!(self.peek(), Tok::Sym("("));
        if call && (lower == "v" || lower == "i") {
            self.bump();
            let mut names = Vec::new();
            loop {
                match self.bump() {
                    Tok::Name(n) => names.push(n),
                    t => return Err(error_at(at, format!("{word}( ) needs names, found {}", describe(&t)))),
                }
                if self.eat(")") {
                    break;
                }
                self.expect(",")?;
            }
            return match (lower.as_str(), names.len()) {
                ("v", 1) => Ok(Expr::Volt(names.remove(0), None)),
                ("v", 2) => {
                    let b = names.pop();
                    Ok(Expr::Volt(names.remove(0), b))
                }
                ("i", 1) => Ok(Expr::Amp(names.remove(0))),
                _ => Err(error_at(
                    at,
                    format!("{word}( ) takes {}", if lower == "v" { "one or two names" } else { "one name" }),
                )),
            };
        }
        if call {
            self.bump();
            self.enter()?;
            let mut args = Vec::new();
            if !self.eat(")") {
                loop {
                    args.push(self.expr()?);
                    if self.eat(")") {
                        break;
                    }
                    self.expect(",")?;
                }
            }
            self.depth -= 1;
            let name = if spice { lower } else { word };
            if name == "if" {
                if args.len() != 3 {
                    return Err(error_at(at, "if(condition, then, else) takes 3 arguments"));
                }
                let b = args.pop().expect("3");
                let a = args.pop().expect("2");
                let c = args.pop().expect("1");
                return Ok(Expr::Cond(Box::new(c), Box::new(a), Box::new(b)));
            }
            return Ok(Expr::Call(name, args));
        }
        let key = if spice { lower.as_str() } else { word.as_str() };
        Ok(match key {
            "pi" => Expr::Pi,
            "time" => Expr::Time,
            "temper" if spice => Expr::Temper,
            _ => Expr::Var(if spice { lower } else { word }),
        })
    }
}

// ---------------------------------------------------------------- user functions

/// A user function (`funcs:` in value rules, `.func` in a SPICE model): parameters and body.
#[derive(Clone, Debug, PartialEq)]
pub struct Func {
    pub params: Vec<String>,
    pub body: Expr,
}

/// Parse a function head (`corner(q)` or `f(a, b)`) into its name and parameter names.
pub fn parse_head(text: &str) -> Result<(String, Vec<String>), ExprError> {
    let t = text.trim();
    let (name, rest) = t.split_once('(').ok_or_else(|| error(format!("\"{t}\" is not name(params)")))?;
    let params = rest.strip_suffix(')').ok_or_else(|| error(format!("\"{t}\" is not name(params)")))?;
    let ok = |s: &str| {
        !s.is_empty()
            && s.chars().next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
            && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
    };
    let name = name.trim().to_string();
    if !ok(&name) {
        return Err(error(format!("\"{name}\" is not a function name")));
    }
    let params: Vec<String> =
        if params.trim().is_empty() { Vec::new() } else { params.split(',').map(|p| p.trim().to_string()).collect() };
    for p in &params {
        if !ok(p) {
            return Err(error(format!("\"{p}\" is not a parameter name")));
        }
    }
    Ok((name, params))
}

/// Replace calls of user functions by their bodies, arguments substituted. Functions may call
/// only functions defined before them, so this terminates.
pub fn inline(e: &Expr, funcs: &BTreeMap<String, Func>) -> Result<Expr, ExprError> {
    Ok(match e {
        Expr::Call(name, args) if funcs.contains_key(name) => {
            let f = &funcs[name];
            if f.params.len() != args.len() {
                return Err(error(format!("{name} takes {} arguments", f.params.len())));
            }
            let args: Vec<Expr> = args.iter().map(|a| inline(a, funcs)).collect::<Result<_, _>>()?;
            let bind: BTreeMap<&str, &Expr> = f.params.iter().map(String::as_str).zip(args.iter()).collect();
            substitute(&f.body, &|v| bind.get(v).map(|x| (*x).clone()))
        }
        _ => map_children(e, &mut |c| inline(c, funcs))?,
    })
}

/// `e` with each variable `index` knows bound to its slot, so evaluation reads a vector instead
/// of looking a name up (value rules evaluate their steps up to 10⁵ times).
pub fn bind(e: &Expr, index: &dyn Fn(&str) -> Option<usize>) -> Expr {
    match e {
        Expr::Var(v) => index(v).map_or_else(|| e.clone(), Expr::Slot),
        _ => map_children(e, &mut |c| Ok::<_, ExprError>(bind(c, index))).expect("infallible"),
    }
}

/// `e` with each variable `subst` knows replaced.
pub fn substitute(e: &Expr, subst: &dyn Fn(&str) -> Option<Expr>) -> Expr {
    match e {
        Expr::Var(v) => subst(v).unwrap_or_else(|| e.clone()),
        _ => map_children(e, &mut |c| Ok::<_, ExprError>(substitute(c, subst))).expect("infallible"),
    }
}

fn map_children<E>(e: &Expr, f: &mut impl FnMut(&Expr) -> Result<Expr, E>) -> Result<Expr, E> {
    Ok(match e {
        Expr::Unary(op, a) => Expr::Unary(*op, Box::new(f(a)?)),
        Expr::Bin(op, a, b) => Expr::Bin(*op, Box::new(f(a)?), Box::new(f(b)?)),
        Expr::Call(n, args) => Expr::Call(n.clone(), args.iter().map(&mut *f).collect::<Result<_, _>>()?),
        Expr::Cond(c, a, b) => Expr::Cond(Box::new(f(c)?), Box::new(f(a)?), Box::new(f(b)?)),
        Expr::List(items) => Expr::List(items.iter().map(&mut *f).collect::<Result<_, _>>()?),
        _ => e.clone(),
    })
}

/// Every node of the tree, depth first.
pub fn walk<'a>(e: &'a Expr, f: &mut impl FnMut(&'a Expr)) {
    f(e);
    match e {
        Expr::Unary(_, a) => walk(a, f),
        Expr::Bin(_, a, b) => {
            walk(a, f);
            walk(b, f);
        }
        Expr::Call(_, args) | Expr::List(args) => args.iter().for_each(|a| walk(a, f)),
        Expr::Cond(c, a, b) => {
            walk(c, f);
            walk(a, f);
            walk(b, f);
        }
        _ => {}
    }
}

/// Whether the value can change during a simulation (it reads a voltage, a current or time).
pub fn is_dynamic(e: &Expr) -> bool {
    let mut dynamic = false;
    walk(e, &mut |x| {
        if matches!(x, Expr::Volt(..) | Expr::Amp(_) | Expr::Time | Expr::Temper) {
            dynamic = true;
        }
    });
    dynamic
}

// ---------------------------------------------------------------- dimensional check

/// What names mean where an expression is checked.
pub trait Scope {
    /// The type of a named value; `None` for an unknown name.
    fn var(&self, name: &str) -> Option<Ty>;
    /// Whether `V(name)` may be read.
    fn node(&self, _name: &str) -> bool {
        false
    }
    /// Whether `I(name)` may be read.
    fn current(&self, _name: &str) -> bool {
        false
    }
    /// Whether `time` may be read.
    fn time(&self) -> bool {
        false
    }
    /// Whether `nearest`/`search` may be called (value rules).
    fn series(&self) -> bool {
        false
    }
    /// Whether `tf` may be called here (the top of a law only).
    fn tf(&self) -> bool {
        false
    }
}

pub fn check(e: &Expr, dialect: Dialect, scope: &dyn Scope) -> Result<Ty, ExprError> {
    Checker { dialect, scope, top: true }.ty(e)
}

struct Checker<'a> {
    dialect: Dialect,
    scope: &'a dyn Scope,
    top: bool,
}

fn show(e: &Expr) -> String {
    let s = e.to_string();
    if s.chars().count() > 40 { format!("{}…", s.chars().take(40).collect::<String>()) } else { s }
}

impl Checker<'_> {
    fn unify(&self, what: &str, a: (Ty, &Expr), b: (Ty, &Expr)) -> Result<Ty, ExprError> {
        match (a.0, b.0) {
            (Ty::Free, t) | (t, Ty::Free) => Ok(t),
            (Ty::Dim(x), Ty::Dim(y)) if x == y => Ok(a.0),
            (Ty::Dim(x), Ty::Dim(y)) => {
                Err(error(format!("{what} {} ({}) and {} ({})", show(a.1), x.describe(), show(b.1), y.describe())))
            }
        }
    }

    fn unitless(&self, t: Ty, e: &Expr, why: &str) -> Result<(), ExprError> {
        match t {
            Ty::Dim(d) if d != Dim::NONE => {
                Err(error(format!("{why} needs a plain number, but {} is in {}", show(e), d.describe())))
            }
            _ => Ok(()),
        }
    }

    fn ty(&mut self, e: &Expr) -> Result<Ty, ExprError> {
        let top = std::mem::replace(&mut self.top, false);
        // Vendor models carry no units: in SPICE's dialect a reading is a plain number, and
        // only names, functions and their arguments are checked.
        if self.dialect == Dialect::Spice && matches!(e, Expr::Volt(..) | Expr::Amp(_) | Expr::Time) {
            return Ok(Ty::Free);
        }
        Ok(match e {
            Expr::Num(_, None) | Expr::Pi => Ty::Free,
            Expr::Num(_, Some(u)) => Ty::Dim(Dim::of(*u)),
            Expr::Var(v) => self.scope.var(v).ok_or_else(|| error(format!("unknown name \"{v}\"")))?,
            Expr::Volt(a, b) => {
                for n in std::iter::once(a).chain(b) {
                    if !self.scope.node(n) {
                        return Err(error(format!("V({n}): there is no pin or node {n} here")));
                    }
                }
                Ty::Dim(Dim::of(Unit::Volt))
            }
            Expr::Amp(n) => {
                if !self.scope.current(n) {
                    return Err(error(format!("I({n}): there is no current {n} to read here")));
                }
                Ty::Dim(Dim::of(Unit::Ampere))
            }
            Expr::Time => {
                if !self.scope.time() {
                    return Err(error("time can only be read inside a law"));
                }
                Ty::Dim(Dim::of(Unit::Second))
            }
            Expr::Temper => Ty::Free,
            Expr::Unary(UnOp::Neg, a) => self.ty(a)?,
            Expr::Unary(UnOp::Not, a) => {
                let t = self.ty(a)?;
                self.unitless(t, a, "!")?;
                Ty::unitless()
            }
            Expr::Bin(op, a, b) => {
                let (ta, tb) = (self.ty(a)?, self.ty(b)?);
                match op {
                    BinOp::Add | BinOp::Sub => {
                        self.unify(if *op == BinOp::Add { "cannot add" } else { "cannot subtract" }, (ta, a), (tb, b))?
                    }
                    BinOp::Mul | BinOp::Div => match (ta, tb) {
                        (Ty::Free, Ty::Free) => Ty::Free,
                        _ => {
                            let rb = if *op == BinOp::Div { tb.dim().inv() } else { tb.dim() };
                            Ty::Dim(ta.dim().mul(rb).ok_or_else(|| error("dimension too large"))?)
                        }
                    },
                    BinOp::Pow => {
                        self.unitless(tb, b, "an exponent")?;
                        match ta {
                            Ty::Dim(d) if d != Dim::NONE => {
                                let n = const_int(b).ok_or_else(|| {
                                    error(format!(
                                        "{} is in {}: its power must be a whole number",
                                        show(a),
                                        d.describe()
                                    ))
                                })?;
                                Ty::Dim(d.pow(n).ok_or_else(|| error("dimension too large"))?)
                            }
                            Ty::Free if tb == Ty::Free => Ty::Free,
                            _ => Ty::unitless(),
                        }
                    }
                    BinOp::And | BinOp::Or => {
                        self.unitless(ta, a, op.symbol())?;
                        self.unitless(tb, b, op.symbol())?;
                        Ty::unitless()
                    }
                    _ => {
                        self.unify("cannot compare", (ta, a), (tb, b))?;
                        Ty::unitless()
                    }
                }
            }
            Expr::Cond(c, a, b) => {
                let tc = self.ty(c)?;
                self.unitless(tc, c, "a condition")?;
                let (ta, tb) = (self.ty(a)?, self.ty(b)?);
                self.unify("the two branches differ:", (ta, a), (tb, b))?
            }
            Expr::List(_) => return Err(error("a list [ ] is only allowed in tf(x, [num], [den])")),
            Expr::Slot(_) => return Err(error("a bound expression is checked before binding")),
            Expr::Call(name, args) => self.call(name, args, top)?,
        })
    }

    fn call(&mut self, name: &str, args: &[Expr], top: bool) -> Result<Ty, ExprError> {
        let (lo, hi, shape) = builtin(name, self.dialect).ok_or_else(|| error(format!("unknown function {name}")))?;
        if args.len() < lo || args.len() > hi {
            let n = if lo == hi { format!("{lo}") } else { format!("{lo} to {hi}") };
            return Err(error(format!("{name} takes {n} arguments")));
        }
        match shape {
            Shape::Series => {
                if !self.scope.series() {
                    return Err(error(format!("{name} is only for value rules")));
                }
                if !matches!(&args[0], Expr::Var(s) if series_of(s).is_some()) {
                    return Err(error(format!("{name}'s first argument is a series: E6, E12 or E24")));
                }
                let mut t = Ty::Free;
                for a in &args[1..] {
                    let ta = self.ty(a)?;
                    t = self.unify(&format!("{name}: cannot mix"), (t, a), (ta, a))?;
                }
                return Ok(t);
            }
            Shape::Tf => {
                if !(top && self.scope.tf()) {
                    return Err(error("tf(x, [num], [den]) must be a whole law's right side, or clamped"));
                }
                let tx = self.ty(&args[0])?;
                for (i, list) in args[1..].iter().enumerate() {
                    let Expr::List(items) = list else {
                        return Err(error("tf(x, [num], [den]): coefficients go in [ ]"));
                    };
                    if items.is_empty() || (i == 1 && items.len() < 2) {
                        return Err(error("tf needs at least one numerator and two denominator coefficients"));
                    }
                    for it in items {
                        if is_dynamic(it) {
                            return Err(error("tf coefficients must be constant"));
                        }
                        // A coefficient of s^k is in seconds^k (relative to the constant term).
                        self.ty(it)?;
                    }
                }
                return Ok(tx);
            }
            _ => {}
        }
        let ts: Vec<Ty> = args.iter().map(|a| self.ty(a)).collect::<Result<_, _>>()?;
        Ok(match shape {
            Shape::Plain => {
                self.unitless(ts[0], &args[0], name)?;
                if ts[0] == Ty::Free { Ty::Free } else { Ty::unitless() }
            }
            Shape::Same => ts[0],
            Shape::Sign => Ty::unitless(),
            Shape::Sqrt => match ts[0] {
                Ty::Free => Ty::Free,
                Ty::Dim(d) => Ty::Dim(
                    d.half()
                        .ok_or_else(|| error(format!("sqrt of {} ({}) has no unit", show(&args[0]), d.describe())))?,
                ),
            },
            Shape::Unify => {
                let mut t = ts[0];
                for (ta, a) in ts.iter().zip(args).skip(1) {
                    t = self.unify(&format!("{name}: cannot mix"), (t, &args[0]), (*ta, a))?;
                }
                t
            }
            Shape::Pow => {
                self.unitless(ts[1], &args[1], "an exponent")?;
                match ts[0] {
                    Ty::Dim(d) if d != Dim::NONE => {
                        let n = const_int(&args[1])
                            .ok_or_else(|| error(format!("{} has a unit: its power must be whole", show(&args[0]))))?;
                        Ty::Dim(d.pow(n).ok_or_else(|| error("dimension too large"))?)
                    }
                    Ty::Free if ts[1] == Ty::Free => Ty::Free,
                    _ => Ty::unitless(),
                }
            }
            Shape::Table => {
                if args.len() % 2 == 0 {
                    return Err(error("table(x, x1, y1, x2, y2, ...) takes x and pairs"));
                }
                let mut tx = ts[0];
                let mut ty = Ty::Free;
                let mut last = f64::NEG_INFINITY;
                for (k, pair) in args[1..].chunks(2).enumerate() {
                    let xk = const_value(&pair[0]).ok_or_else(|| error("table points must be constant numbers"))?;
                    if xk <= last {
                        return Err(error("table points must increase"));
                    }
                    last = xk;
                    tx = self.unify("table: x and its points differ:", (tx, &args[0]), (ts[1 + 2 * k], &pair[0]))?;
                    ty = self.unify("table: values differ:", (ty, &pair[1]), (ts[2 + 2 * k], &pair[1]))?;
                }
                ty
            }
            Shape::If | Shape::Series | Shape::Tf => unreachable!("handled above"),
        })
    }
}

/// The value of a constant whole number (`2`, `-3`), for powers of quantities with units.
fn const_int(e: &Expr) -> Option<i8> {
    let v = const_value(e)?;
    (v.fract() == 0.0 && v.abs() <= 16.0).then_some(v as i8)
}

/// The value of an expression made of numbers only.
pub fn const_value(e: &Expr) -> Option<f64> {
    struct NoVars;
    impl Values for NoVars {
        fn var(&self, _: &str) -> Option<f64> {
            None
        }
    }
    eval(e, &NoVars).ok().map(|v| v.value)
}

/// `E6`, `E12`, `E24` by name.
pub fn series_of(name: &str) -> Option<crate::units::ESeries> {
    use crate::units::ESeries;
    match name {
        "E6" => Some(ESeries::E6),
        "E12" => Some(ESeries::E12),
        "E24" => Some(ESeries::E24),
        _ => None,
    }
}

// ---------------------------------------------------------------- evaluation

/// Values an expression reads.
pub trait Values {
    fn var(&self, name: &str) -> Option<f64>;
    fn volt(&self, _a: &str, _b: Option<&str>) -> Option<f64> {
        None
    }
    fn amp(&self, _name: &str) -> Option<f64> {
        None
    }
    fn time(&self) -> Option<f64> {
        None
    }
    fn slot(&self, _k: usize) -> Option<f64> {
        None
    }
}

impl Values for BTreeMap<String, f64> {
    fn var(&self, name: &str) -> Option<f64> {
        self.get(name).copied()
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Eval {
    pub value: f64,
    /// A transcendental function or a fractional power contributed: the last bits may differ
    /// between libm builds.
    pub inexact: bool,
}

/// ngspice's default circuit temperature.
const TEMPER: f64 = 27.0;

pub fn eval(e: &Expr, v: &dyn Values) -> Result<Eval, ExprError> {
    let exact = |value| Ok(Eval { value, inexact: false });
    match e {
        Expr::Num(x, _) => exact(*x),
        Expr::Pi => exact(std::f64::consts::PI),
        Expr::Temper => exact(TEMPER),
        Expr::Var(n) => {
            v.var(n).map(|x| Eval { value: x, inexact: false }).ok_or_else(|| error(format!("{n} has no value")))
        }
        Expr::Slot(k) => {
            v.slot(*k).map(|x| Eval { value: x, inexact: false }).ok_or_else(|| error("a slot has no value"))
        }
        Expr::Volt(a, b) => v
            .volt(a, b.as_deref())
            .map(|x| Eval { value: x, inexact: false })
            .ok_or_else(|| error("a voltage has no value here")),
        Expr::Amp(n) => v
            .amp(n)
            .map(|x| Eval { value: x, inexact: false })
            .ok_or_else(|| error(format!("I({n}) has no value here"))),
        Expr::Time => {
            v.time().map(|x| Eval { value: x, inexact: false }).ok_or_else(|| error("time has no value here"))
        }
        Expr::Unary(op, a) => {
            let r = eval(a, v)?;
            Ok(Eval {
                value: match op {
                    UnOp::Neg => -r.value,
                    UnOp::Not => truth(r.value == 0.0),
                },
                inexact: r.inexact,
            })
        }
        Expr::Bin(op, a, b) => {
            let (x, y) = (eval(a, v)?, eval(b, v)?);
            let inexact = x.inexact || y.inexact;
            if *op == BinOp::Pow {
                let (value, frac) = power(x.value, y.value);
                return Ok(Eval { value, inexact: inexact || frac });
            }
            let (p, q) = (x.value, y.value);
            Ok(Eval {
                value: match op {
                    BinOp::Add => p + q,
                    BinOp::Sub => p - q,
                    BinOp::Mul => p * q,
                    BinOp::Div => p / q,
                    BinOp::Lt => truth(p < q),
                    BinOp::Le => truth(p <= q),
                    BinOp::Gt => truth(p > q),
                    BinOp::Ge => truth(p >= q),
                    BinOp::Eq => truth(p == q),
                    BinOp::Ne => truth(p != q),
                    BinOp::And => truth(p != 0.0 && q != 0.0),
                    BinOp::Or => truth(p != 0.0 || q != 0.0),
                    BinOp::Pow => unreachable!(),
                },
                inexact,
            })
        }
        Expr::Cond(c, a, b) => {
            let r = eval(c, v)?;
            let out = if r.value != 0.0 { eval(a, v)? } else { eval(b, v)? };
            Ok(Eval { value: out.value, inexact: out.inexact || r.inexact })
        }
        Expr::List(_) => Err(error("a list has no single value")),
        Expr::Call(name, args) => call(name, args, v),
    }
}

fn truth(b: bool) -> f64 {
    if b { 1.0 } else { 0.0 }
}

/// `x^y`: whole exponents as repeated products (identical everywhere), others through `powf`.
fn power(x: f64, y: f64) -> (f64, bool) {
    if y.fract() == 0.0 && y.abs() <= MAX_INT_POW {
        let n = y.abs() as u32;
        let mut p = 1.0;
        for k in 0..n {
            p = if k == 0 { x } else { p * x };
        }
        (if y < 0.0 { 1.0 / p } else { p }, false)
    } else {
        (x.powf(y), true)
    }
}

/// Piecewise-linear interpolation through `(xs, ys)`, constant beyond the ends.
fn table_at(x: f64, pts: &[(f64, f64)]) -> f64 {
    let (x0, y0) = pts[0];
    if x < x0 {
        return y0;
    }
    for w in pts.windows(2) {
        let ((xa, ya), (xb, yb)) = (w[0], w[1]);
        if x < xb {
            return ya + (yb - ya) * (x - xa) / (xb - xa);
        }
    }
    pts[pts.len() - 1].1
}

fn call(name: &str, args: &[Expr], v: &dyn Values) -> Result<Eval, ExprError> {
    if name == "tf" || name == "nearest" || name == "search" {
        return Err(error(format!("{name} has no value by itself")));
    }
    let rs: Vec<Eval> = args.iter().map(|a| eval(a, v)).collect::<Result<_, _>>()?;
    let mut inexact = rs.iter().any(|r| r.inexact) || transcendental(name);
    let x: Vec<f64> = rs.iter().map(|r| r.value).collect();
    let value = match (name, x.as_slice()) {
        ("exp", [a]) => a.exp(),
        ("ln" | "log", [a]) => a.ln(),
        ("log10", [a]) => a.log10(),
        ("tanh", [a]) => a.tanh(),
        ("sin", [a]) => a.sin(),
        ("cos", [a]) => a.cos(),
        ("tan", [a]) => a.tan(),
        ("atan", [a]) => a.atan(),
        ("asin", [a]) => a.asin(),
        ("acos", [a]) => a.acos(),
        ("sinh", [a]) => a.sinh(),
        ("cosh", [a]) => a.cosh(),
        ("sqrt", [a]) => a.sqrt(),
        ("abs", [a]) => a.abs(),
        ("floor", [a]) => a.floor(),
        ("ceil", [a]) => a.ceil(),
        ("sgn", [a]) => {
            if *a > 0.0 {
                1.0
            } else if *a < 0.0 {
                -1.0
            } else {
                0.0
            }
        }
        ("u", [a]) => truth(*a > 0.0),
        ("uramp", [a]) => a.max(0.0),
        ("min", [a, b]) => a.min(*b),
        ("max", [a, b]) => a.max(*b),
        ("clamp", [a, lo, hi]) => a.max(*lo).min(*hi),
        ("limit", [a, lo, hi]) => a.max(lo.min(*hi)).min(lo.max(*hi)),
        ("pow", [a, b]) => {
            let (p, frac) = power(*a, *b);
            inexact |= frac;
            p
        }
        ("pwr", [a, b]) => {
            let (p, frac) = power(a.abs(), *b);
            inexact |= frac;
            p
        }
        ("pwrs", [a, b]) => {
            let (p, frac) = power(a.abs(), *b);
            inexact |= frac;
            if *a < 0.0 { -p } else { p }
        }
        ("table", [a, rest @ ..]) => {
            let pts: Vec<(f64, f64)> = rest.chunks(2).map(|c| (c[0], c[1])).collect();
            table_at(*a, &pts)
        }
        _ => return Err(error(format!("{name} cannot be evaluated with {} arguments", args.len()))),
    };
    Ok(Eval { value, inexact })
}

// ---------------------------------------------------------------- SPICE emission

/// How names inside an expression become SPICE text.
pub trait Names {
    /// A named value: normally a number already ([`spice_number`]).
    fn var(&self, name: &str) -> Option<String>;
    fn node(&self, _name: &str) -> Option<String> {
        None
    }
    /// The voltage source whose current `I(name)` reads.
    fn current(&self, _name: &str) -> Option<String> {
        None
    }
}

/// ngspice B-source text for `e`. Every operation is parenthesised, so no precedence rule of
/// any ngspice parser matters; whole powers are written as products, `clamp`, `limit`, `pwr`
/// and `table` in terms of `min`, `max`, `abs` and `?:`.
pub fn emit(e: &Expr, names: &dyn Names) -> Result<String, ExprError> {
    Ok(match e {
        Expr::Num(x, _) => num(*x),
        Expr::Pi => num(std::f64::consts::PI),
        Expr::Temper => "temper".into(),
        Expr::Time => "time".into(),
        Expr::Var(n) => names.var(n).ok_or_else(|| error(format!("{n} has no value")))?,
        Expr::Volt(a, b) => {
            let na = names.node(a).ok_or_else(|| error(format!("no node {a}")))?;
            match b {
                Some(b) => format!("v({na},{})", names.node(b).ok_or_else(|| error(format!("no node {b}")))?),
                None => format!("v({na})"),
            }
        }
        Expr::Amp(n) => format!("i({})", names.current(n).ok_or_else(|| error(format!("no current {n}")))?),
        Expr::Unary(UnOp::Neg, a) => format!("(-{})", emit(a, names)?),
        Expr::Unary(UnOp::Not, a) => format!("(!{})", emit(a, names)?),
        Expr::Bin(BinOp::Pow, a, b) => power_text(emit(a, names)?, b, names)?,
        Expr::Bin(op, a, b) => format!("({}{}{})", emit(a, names)?, op.symbol(), emit(b, names)?),
        Expr::Cond(c, a, b) => format!("({} ? {} : {})", emit(c, names)?, emit(a, names)?, emit(b, names)?),
        Expr::List(_) => return Err(error("a list cannot be emitted")),
        Expr::Slot(_) => return Err(error("a bound expression cannot be emitted")),
        Expr::Call(name, args) => {
            let xs: Vec<String> = args.iter().map(|a| emit(a, names)).collect::<Result<_, _>>()?;
            match (name.as_str(), xs.as_slice()) {
                ("clamp", [a, lo, hi]) => format!("min(max({a},{lo}),{hi})"),
                ("limit", [a, lo, hi]) => format!("min(max({a},min({lo},{hi})),max({lo},{hi}))"),
                ("pow", [a, _]) => power_text(a.clone(), &args[1], names)?,
                ("pwr", [a, _]) => power_text(format!("abs({a})"), &args[1], names)?,
                ("pwrs", [a, _]) => {
                    let p = power_text(format!("abs({a})"), &args[1], names)?;
                    format!("(({a}<0) ? (-{p}) : {p})")
                }
                ("log", [a]) => format!("ln({a})"),
                ("sgn", [a]) => format!("(({a}>0) ? 1 : (({a}<0) ? (-1) : 0))"),
                ("u", [a]) => format!("(({a}>0) ? 1 : 0)"),
                ("uramp", [a]) => format!("max({a},0)"),
                ("table", [x, rest @ ..]) => {
                    let pts: Vec<(&String, &String)> = rest.chunks(2).map(|c| (&c[0], &c[1])).collect();
                    let (x0, y0) = pts[0];
                    let mut out = pts[pts.len() - 1].1.clone();
                    for w in pts.windows(2).rev() {
                        let ((xa, ya), (xb, yb)) = (w[0], w[1]);
                        out = format!("(({x}<{xb}) ? ({ya}+(({yb}-{ya})*({x}-{xa}))/({xb}-{xa})) : {out})");
                    }
                    format!("(({x}<{x0}) ? {y0} : {out})")
                }
                _ => {
                    if builtin(name, Dialect::Spice).is_none() && builtin(name, Dialect::Law).is_none() {
                        return Err(error(format!("unknown function {name}")));
                    }
                    format!("{name}({})", xs.join(","))
                }
            }
        }
    })
}

/// A number as ngspice reads it; negative numbers parenthesised.
fn num(x: f64) -> String {
    if x < 0.0 { format!("({})", spice_number(x)) } else { spice_number(x) }
}

/// `base^exp`: a whole constant exponent as a product (as [`power`] computes it), else `pow`.
fn power_text(base: String, exp: &Expr, names: &dyn Names) -> Result<String, ExprError> {
    Ok(match const_value(exp) {
        Some(y) if y.fract() == 0.0 && y.abs() <= MAX_INT_POW => {
            let n = y.abs() as usize;
            let prod = if n == 0 { "1".to_string() } else { format!("({})", vec![base.as_str(); n].join("*")) };
            if y < 0.0 { format!("(1/{prod})") } else { prod }
        }
        _ => format!("pow({base},{})", emit(exp, names)?),
    })
}

// ---------------------------------------------------------------- display

impl fmt::Display for Expr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write_expr(f, self, 0)
    }
}

fn write_expr(f: &mut fmt::Formatter<'_>, e: &Expr, outer: u8) -> fmt::Result {
    match e {
        Expr::Num(x, u) => {
            let unit = u.map_or("", |u| u.symbol());
            if *x < 0.0 && outer > 0 { write!(f, "({x}{unit})") } else { write!(f, "{x}{unit}") }
        }
        Expr::Var(v) => write!(f, "{v}"),
        Expr::Slot(k) => write!(f, "#{k}"),
        Expr::Volt(a, None) => write!(f, "V({a})"),
        Expr::Volt(a, Some(b)) => write!(f, "V({a}, {b})"),
        Expr::Amp(a) => write!(f, "I({a})"),
        Expr::Time => write!(f, "time"),
        Expr::Pi => write!(f, "pi"),
        Expr::Temper => write!(f, "temper"),
        Expr::Unary(op, a) => {
            let paren = outer > 6;
            if paren {
                write!(f, "(")?;
            }
            write!(f, "{}", if *op == UnOp::Neg { "-" } else { "!" })?;
            write_expr(f, a, 6)?;
            if paren {
                write!(f, ")")?;
            }
            Ok(())
        }
        Expr::Bin(op, a, b) => {
            let p = op.precedence();
            let paren = p < outer;
            if paren {
                write!(f, "(")?;
            }
            // Left-associative: the right operand binds tighter; `^` the other way; comparisons
            // do not chain, so both sides bind tighter.
            let (pl, pr) = match op {
                BinOp::Pow => (p + 1, p),
                _ if op.is_comparison() => (p + 1, p + 1),
                _ => (p, p + 1),
            };
            write_expr(f, a, pl)?;
            write!(f, " {} ", op.symbol())?;
            write_expr(f, b, pr)?;
            if paren {
                write!(f, ")")?;
            }
            Ok(())
        }
        Expr::Cond(c, a, b) => {
            write!(f, "if(")?;
            write_expr(f, c, 0)?;
            write!(f, ", ")?;
            write_expr(f, a, 0)?;
            write!(f, ", ")?;
            write_expr(f, b, 0)?;
            write!(f, ")")
        }
        Expr::Call(n, args) => {
            write!(f, "{n}(")?;
            for (i, a) in args.iter().enumerate() {
                if i > 0 {
                    write!(f, ", ")?;
                }
                write_expr(f, a, 0)?;
            }
            write!(f, ")")
        }
        Expr::List(items) => {
            write!(f, "[")?;
            for (i, a) in items.iter().enumerate() {
                if i > 0 {
                    write!(f, ", ")?;
                }
                write_expr(f, a, 0)?;
            }
            write!(f, "]")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Pins;
    impl Scope for Pins {
        fn var(&self, name: &str) -> Option<Ty> {
            match name {
                "gain" => Some(Ty::unitless()),
                "fc" => Some(Ty::Dim(Dim::of(Unit::Hertz))),
                "R1" => Some(Ty::Dim(Dim::of(Unit::Ohm))),
                "C1" => Some(Ty::Dim(Dim::of(Unit::Farad))),
                _ => None,
            }
        }
        fn node(&self, name: &str) -> bool {
            ["in", "out", "vcc", "vee", "ref", "inp", "inm"].contains(&name)
        }
        fn current(&self, name: &str) -> bool {
            name == "out"
        }
        fn time(&self) -> bool {
            true
        }
    }

    fn ty(s: &str) -> Result<Ty, ExprError> {
        check(&parse(s, Dialect::Law)?, Dialect::Law, &Pins)
    }

    fn val(s: &str) -> f64 {
        eval(&parse(s, Dialect::Law).unwrap(), &BTreeMap::new()).unwrap().value
    }

    #[test]
    fn numbers_read_like_units() {
        assert_eq!(val("4.7k"), 4700.0);
        assert_eq!(val("4k7"), 4700.0);
        assert_eq!(val("100p"), 1e-10);
        assert_eq!(val("10u"), 1e-5);
        assert_eq!(val("1meg"), 1e6);
        assert_eq!(val("2.5m"), 2.5e-3);
        assert_eq!(val("1.5V"), 1.5);
        assert_eq!(val("10kΩ"), 1e4);
        assert_eq!(parse("1.5V", Dialect::Law).unwrap(), Expr::Num(1.5, Some(Unit::Volt)));
        assert!(parse("2pi", Dialect::Law).unwrap_err().message.contains("2*pi"));
        let spice = |s: &str| eval(&parse(s, Dialect::Spice).unwrap(), &BTreeMap::new()).unwrap().value;
        assert_eq!(spice("1M"), 1e-3);
        assert_eq!(spice("1MEG"), 1e6);
        assert_eq!(spice("10uF"), 1e-5);
        assert_eq!(spice("10V"), 10.0);
    }

    #[test]
    fn precedence_and_associativity() {
        assert_eq!(val("2 + 3 * 4"), 14.0);
        assert_eq!(val("2 ^ 3 ^ 2"), 512.0);
        assert_eq!(val("-2 ^ 2"), -4.0);
        assert_eq!(val("8 / 4 / 2"), 1.0);
        assert_eq!(val("1 < 2 ? 5 : 6"), 5.0);
        assert_eq!(val("if(0, 5, 6)"), 6.0);
        assert_eq!(val("clamp(5, 0, 3)"), 3.0);
        assert_eq!(val("table(1.5, 1, 10, 2, 20)"), 15.0);
        assert_eq!(val("table(0, 1, 10, 2, 20)"), 10.0);
        assert_eq!(val("2 × 3 − 1"), 5.0);
        assert!(parse("1 < 2 < 3", Dialect::Law).is_err());
    }

    #[test]
    fn dimensions() {
        assert_eq!(ty("V(in) * gain").unwrap(), Ty::Dim(Dim::of(Unit::Volt)));
        assert_eq!(ty("V(vcc) - 1.5").unwrap(), Ty::Dim(Dim::of(Unit::Volt)));
        assert_eq!(ty("R1 * C1").unwrap(), Ty::Dim(Dim::of(Unit::Second)));
        assert_eq!(ty("1/(2*pi*R1*C1)").unwrap(), Ty::Dim(Dim::of(Unit::Hertz)));
        assert_eq!(ty("V(out)/I(out)").unwrap(), Ty::Dim(Dim::of(Unit::Ohm)));
        assert_eq!(ty("sqrt(R1*R1)").unwrap(), Ty::Dim(Dim::of(Unit::Ohm)));
        assert_eq!(ty("V(in)^2/R1").unwrap(), Ty::Dim(Dim::of(Unit::Watt)));
        assert_eq!(ty("sin(2*pi*fc*time)").unwrap(), Ty::unitless());
        let e = ty("V(in) + I(out)").unwrap_err().message;
        assert_eq!(e, "cannot add V(in) (volts) and I(out) (amps)");
        assert!(ty("V(ref) + gain").unwrap_err().message.contains("a plain number"));
        assert!(ty("exp(V(in))").unwrap_err().message.contains("needs a plain number"));
        assert!(ty("V(nowhere)").unwrap_err().message.contains("no pin or node"));
        assert!(ty("foo(1)").unwrap_err().message.contains("unknown function"));
        assert!(ty("sqrt(R1)").is_err());
        assert!(ty("V(in)^gain").is_err());
        assert!(ty("nearest(E24, R1)").unwrap_err().message.contains("value rules"));
        let spice = |s: &str| check(&parse(s, Dialect::Spice).unwrap(), Dialect::Spice, &Pins);
        assert_eq!(spice("tanh((v(in) - 0.7)/0.05) + i(out)").unwrap(), Ty::Free, "no units in vendor models");
    }

    #[test]
    fn whole_powers_are_products() {
        assert_eq!(power(3.0, 2.0), (9.0, false));
        assert_eq!(power(2.0, -2.0), (0.25, false));
        assert!(power(2.0, 0.5).1);
        let e = parse("x^3", Dialect::Law).unwrap();
        struct X;
        impl Names for X {
            fn var(&self, _: &str) -> Option<String> {
                Some("x".into())
            }
        }
        assert_eq!(emit(&e, &X).unwrap(), "(x*x*x)");
        assert!(eval(&parse("exp(1)", Dialect::Law).unwrap(), &BTreeMap::new()).unwrap().inexact);
        assert!(!eval(&parse("sqrt(2)", Dialect::Law).unwrap(), &BTreeMap::new()).unwrap().inexact);
    }

    #[test]
    fn emission_is_parenthesised() {
        struct N;
        impl Names for N {
            fn var(&self, name: &str) -> Option<String> {
                (name == "gain").then(|| spice_number(10.0))
            }
            fn node(&self, name: &str) -> Option<String> {
                Some(name.to_string())
            }
        }
        let e = parse("clamp(V(ref) + gain*(V(inp) - V(inm)), V(vee) + 0.02, V(vcc) - 1.5)", Dialect::Law).unwrap();
        assert_eq!(emit(&e, &N).unwrap(), "min(max((v(ref)+(1e1*(v(inp)-v(inm)))),(v(vee)+2e-2)),(v(vcc)-1.5e0))");
        let e = parse("-V(in) < 0 ? 1 : -2", Dialect::Law).unwrap();
        assert_eq!(emit(&e, &N).unwrap(), "(((-v(in))<0e0) ? 1e0 : (-2e0))");
    }

    #[test]
    fn display_round_trips() {
        for s in [
            "1 + 2 * 3",
            "(1 + 2) * 3",
            "1 - (2 - 3)",
            "2 ^ 3 ^ 2",
            "(2 ^ 3) ^ 2",
            "-x * y",
            "a / (b * c)",
            "if(a < b, a, b)",
            "V(a, b) + I(out)",
        ] {
            let e = parse(s, Dialect::Law).unwrap();
            let again = parse(&e.to_string(), Dialect::Law).unwrap();
            assert_eq!(e, again, "{s} -> {e}");
        }
    }

    #[test]
    fn funcs_inline() {
        let (name, params) = parse_head("corner(q)").unwrap();
        let body = parse("sqrt((2 - 1/(q*q) + sqrt((2 - 1/(q*q))*(2 - 1/(q*q)) + 4))/2)", Dialect::Law).unwrap();
        let funcs = BTreeMap::from([(name, Func { params, body })]);
        let e = inline(&parse("corner(0.5*2)", Dialect::Law).unwrap(), &funcs).unwrap();
        let b: f64 = 2.0 - 1.0 / (1.0 * 1.0);
        let want = ((b + (b * b + 4.0).sqrt()) / 2.0).sqrt();
        assert_eq!(eval(&e, &BTreeMap::new()).unwrap().value, want);
    }

    #[test]
    fn depth_is_bounded() {
        let deep = format!("{}1{}", "(".repeat(200), ")".repeat(200));
        assert!(parse(&deep, Dialect::Law).unwrap_err().message.contains("nest"));
        let neg = format!("{}1", "-".repeat(200));
        assert!(parse(&neg, Dialect::Law).is_err());
    }
}
