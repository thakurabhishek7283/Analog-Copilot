//! SPICE model import (LLD §17 D3): a vendor `.subckt` read by an allow-list parser and written
//! back out from its tree, so no text a user pasted reaches the simulator (LLD §14, "no SPICE
//! injection").
//!
//! Accepted: elements R C L D Q M J K E F G H B V I X, and `.subckt/.ends/.model/.param/.func`
//! (`.end` stops reading). Everything else (`.include`, `.lib`, `.control`, `.options`, analyses,
//! `shell`, transient source functions, `TABLE`/`LAPLACE` sources) is refused with its line
//! number, and so is a file over [`MAX_BYTES`]. Parameters and `{expressions}` are evaluated
//! here, `.func`s inlined, POLY sources rewritten as B-sources (the server's ngspice has no
//! XSPICE, which ngspice's own POLY needs), and every node, element and model renamed, so the
//! emitted text holds only numbers, generated names and expressions printed from their trees.

use std::collections::{BTreeMap, BTreeSet};

use indexmap::IndexMap;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::expr::{self, Dialect, Expr, Func, Names};
use crate::units::spice_number;

pub const MAX_BYTES: usize = 256 * 1024;
pub const MAX_SUBCKTS: usize = 64;
pub const MAX_ELEMENTS: usize = 5000;
/// Highest POLY dimension and degree accepted.
const MAX_POLY_DIMS: usize = 8;

/// A vendor model as the user gave it: the text (parsed, never passed on), which `.subckt` is
/// the component, and which of its ports each component pin is.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct SpiceModel {
    pub text: String,
    pub subckt: String,
    /// Component pin -> port of the `.subckt`; every port is mapped once.
    pub pins: IndexMap<String, String>,
}

// ---------------------------------------------------------------- tree

#[derive(Clone, Debug, PartialEq)]
pub struct Deck {
    pub params: Vec<(String, Expr)>,
    pub funcs: Vec<(String, Func)>,
    pub models: Vec<ModelCard>,
    pub subckts: Vec<Subckt>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Subckt {
    pub name: String,
    pub ports: Vec<String>,
    /// `PARAMS:` defaults.
    pub defaults: Vec<(String, Expr)>,
    pub params: Vec<(String, Expr)>,
    pub funcs: Vec<(String, Func)>,
    pub models: Vec<ModelCard>,
    pub elements: Vec<Element>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ModelCard {
    pub name: String,
    pub kind: String,
    pub params: Vec<(String, Expr)>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Element {
    /// R, C, L: two nodes, a value, extra `key=value`s.
    Passive { kind: char, name: String, nodes: [String; 2], value: Expr, extra: Vec<(String, Expr)> },
    /// D, Q, M, J: nodes, a model, an optional area, extra `key=value`s.
    Device {
        kind: char,
        name: String,
        nodes: Vec<String>,
        model: String,
        area: Option<Expr>,
        extra: Vec<(String, Expr)>,
    },
    /// K: two inductors and a coupling.
    Coupling { name: String, a: String, b: String, k: Expr },
    /// E, G (`ctrl` node pairs) and F, H (`ctrl` voltage sources) as a linear gain or a POLY.
    Controlled {
        kind: char,
        name: String,
        nodes: [String; 2],
        ctrl: Vec<String>,
        coeffs: Vec<Expr>,
        poly: Option<usize>,
    },
    /// B, and E/G with `VALUE=`: a voltage (`volt`) or current source of an expression.
    Behavioral { name: String, nodes: [String; 2], volt: bool, expr: Expr },
    /// V, I at DC.
    Source { kind: char, name: String, nodes: [String; 2], dc: Expr },
    /// X: a sub-circuit of this file.
    Call { name: String, nodes: Vec<String>, sub: String, params: Vec<(String, Expr)> },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImportError {
    pub line: Option<usize>,
    pub message: String,
}

impl std::fmt::Display for ImportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.line {
            Some(l) => write!(f, "line {l}: {}", self.message),
            None => write!(f, "{}", self.message),
        }
    }
}

fn at(line: usize, message: impl Into<String>) -> ImportError {
    ImportError { line: Some(line), message: message.into() }
}

// ---------------------------------------------------------------- lines and tokens

/// Logical lines (continuations joined, comments dropped) with their first physical line number.
fn logical_lines(text: &str) -> Result<Vec<(usize, String)>, ImportError> {
    let mut out: Vec<(usize, String)> = Vec::new();
    for (i, raw) in text.split('\n').enumerate() {
        let n = i + 1;
        let line = strip_comment(raw.trim_end_matches('\r'));
        let t = line.trim();
        if t.is_empty() {
            continue;
        }
        if let Some(rest) = t.strip_prefix('+') {
            match out.last_mut() {
                Some((_, prev)) => {
                    prev.push(' ');
                    prev.push_str(rest);
                }
                None => return Err(at(n, "a continuation line (+) with nothing before it")),
            }
        } else {
            out.push((n, t.to_string()));
        }
    }
    Ok(out)
}

fn strip_comment(line: &str) -> &str {
    if line.trim_start().starts_with('*') {
        return "";
    }
    let mut end = line.len();
    if let Some(k) = line.find(';') {
        end = end.min(k);
    }
    let b = line.as_bytes();
    for k in 0..b.len() {
        if b[k] == b'$'
            && (k + 1 == b.len() || b[k + 1] == b' ' || b[k + 1] == b'\t')
            && (k == 0 || b[k - 1].is_ascii_whitespace())
        {
            end = end.min(k);
            break;
        }
    }
    &line[..end]
}

#[derive(Clone, Debug, PartialEq)]
enum T {
    Word(String),
    /// `{…}` or `'…'`: an expression.
    Brace(String),
    Eq,
    Open,
    Close,
    Comma,
}

fn tokens(line: &str, n: usize) -> Result<Vec<T>, ImportError> {
    let chars: Vec<char> = line.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        match c {
            _ if c.is_whitespace() => i += 1,
            '=' => {
                out.push(T::Eq);
                i += 1;
            }
            '(' => {
                out.push(T::Open);
                i += 1;
            }
            ')' => {
                out.push(T::Close);
                i += 1;
            }
            ',' => {
                out.push(T::Comma);
                i += 1;
            }
            '{' => {
                let mut depth = 0;
                let s = i;
                loop {
                    match chars.get(i) {
                        Some('{') => depth += 1,
                        Some('}') => {
                            depth -= 1;
                            if depth == 0 {
                                break;
                            }
                        }
                        Some(_) => {}
                        None => return Err(at(n, "an unclosed {")),
                    }
                    i += 1;
                }
                out.push(T::Brace(chars[s + 1..i].iter().collect()));
                i += 1;
            }
            '\'' => {
                let s = i + 1;
                i += 1;
                while i < chars.len() && chars[i] != '\'' {
                    i += 1;
                }
                if i >= chars.len() {
                    return Err(at(n, "an unclosed '"));
                }
                out.push(T::Brace(chars[s..i].iter().collect()));
                i += 1;
            }
            '}' => return Err(at(n, "a } without {")),
            _ => {
                let s = i;
                while i < chars.len() && !chars[i].is_whitespace() && !"=(),{}'".contains(chars[i]) {
                    i += 1;
                }
                out.push(T::Word(chars[s..i].iter().collect()));
            }
        }
    }
    Ok(out)
}

/// The first `n` whitespace-separated words and the rest of the line.
fn head(line: &str, n: usize) -> Option<(Vec<&str>, &str)> {
    let mut words = Vec::new();
    let mut rest = line.trim_start();
    for _ in 0..n {
        let end = rest.find(char::is_whitespace).unwrap_or(rest.len());
        if end == 0 {
            return None;
        }
        words.push(&rest[..end]);
        rest = rest[end..].trim_start();
    }
    Some((words, rest))
}

fn ident(s: &str) -> bool {
    let mut cs = s.chars();
    cs.next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_') && cs.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

fn node_ok(s: &str) -> bool {
    !s.is_empty() && s.len() <= 64 && s.chars().all(|c| c.is_ascii_alphanumeric() || "_$#.:-+[]<>!/|".contains(c))
}

/// A value: a number, a name or `{expression}`, in SPICE's dialect.
fn value(t: &T, n: usize) -> Result<Expr, ImportError> {
    let text = match t {
        T::Word(w) | T::Brace(w) => w,
        _ => return Err(at(n, "expected a value")),
    };
    expr::parse(text, Dialect::Spice).map_err(|e| at(n, format!("\"{text}\": {}", e.message)))
}

/// `key=value` pairs (keys lowercased), commas and parentheses ignored.
fn pairs(ts: &[T], n: usize) -> Result<Vec<(String, Expr)>, ImportError> {
    let ts: Vec<&T> = ts.iter().filter(|t| !matches!(t, T::Comma | T::Open | T::Close)).collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < ts.len() {
        let (T::Word(k), Some(T::Eq), Some(v)) = (ts[i], ts.get(i + 1), ts.get(i + 2)) else {
            return Err(at(n, "expected name=value"));
        };
        if !ident(k) {
            return Err(at(n, format!("\"{k}\" is not a parameter name")));
        }
        out.push((k.to_ascii_lowercase(), value(v, n)?));
        i += 3;
    }
    Ok(out)
}

/// An expression after `key=` in raw text: braced, quoted or bare to the end of the line.
fn tail_expr(rest: &str, keys: &[&str], n: usize) -> Result<Expr, ImportError> {
    let r = rest.trim();
    let lower = r.to_ascii_lowercase();
    let key = keys
        .iter()
        .find(|k| lower.starts_with(**k))
        .ok_or_else(|| at(n, format!("expected {}=", keys[0].to_ascii_uppercase())))?;
    let r = r[key.len()..].trim_start();
    let r = r.strip_prefix('=').ok_or_else(|| at(n, format!("expected {}=", key.to_ascii_uppercase())))?.trim();
    let body = if let Some(b) = r.strip_prefix('{').and_then(|b| b.strip_suffix('}')) {
        b
    } else if let Some(b) = r.strip_prefix('\'').and_then(|b| b.strip_suffix('\'')) {
        b
    } else {
        r
    };
    expr::parse(body, Dialect::Spice).map_err(|e| at(n, e.message))
}

// ---------------------------------------------------------------- parser

const MODEL_KINDS: [&str; 11] = ["d", "npn", "pnp", "njf", "pjf", "nmos", "pmos", "vdmos", "r", "c", "l"];

fn extra_keys(kind: char) -> &'static [&'static str] {
    match kind {
        'R' => &["tc1", "tc2", "tc", "m"],
        'C' | 'L' => &["ic", "m"],
        'D' => &["m", "area"],
        'Q' => &["m", "area"],
        'M' => &["l", "w", "ad", "as", "pd", "ps", "nrd", "nrs", "m", "nf"],
        'J' => &["m", "area"],
        _ => &[],
    }
}

pub fn parse(text: &str) -> Result<Deck, ImportError> {
    if text.len() > MAX_BYTES {
        return Err(ImportError { line: None, message: format!("a model file is at most {} KB", MAX_BYTES / 1024) });
    }
    let lines = logical_lines(text)?;
    // Model names first: a Q line's 4th word is a node or a model.
    let mut model_names = BTreeSet::new();
    for (_, l) in &lines {
        let mut w = l.split_whitespace();
        if w.next().is_some_and(|c| c.eq_ignore_ascii_case(".model"))
            && let Some(name) = w.next()
        {
            model_names.insert(name.split('(').next().unwrap_or(name).to_ascii_lowercase());
        }
    }
    let mut deck = Deck { params: Vec::new(), funcs: Vec::new(), models: Vec::new(), subckts: Vec::new() };
    let mut open: Option<Subckt> = None;
    // Functions known so far (bodies inlined): the file's, plus the open sub-circuit's.
    let mut file_funcs: BTreeMap<String, Func> = BTreeMap::new();
    let mut sub_funcs: BTreeMap<String, Func> = BTreeMap::new();
    let mut elements = 0;
    for (n, line) in lines {
        let first = line.split_whitespace().next().unwrap_or("");
        let lower = first.to_ascii_lowercase();
        if lower.starts_with('.') {
            match lower.as_str() {
                ".end" => break,
                ".subckt" => {
                    if open.is_some() {
                        return Err(at(n, "a .subckt inside a .subckt is not supported"));
                    }
                    if deck.subckts.len() >= MAX_SUBCKTS {
                        return Err(at(n, format!("at most {MAX_SUBCKTS} sub-circuits")));
                    }
                    let head = parse_subckt_head(&line, n)?;
                    for (_, e) in &head.defaults {
                        check_functions(e, &file_funcs, n)?;
                    }
                    sub_funcs = file_funcs.clone();
                    open = Some(head);
                }
                ".ends" => {
                    let s = open.take().ok_or_else(|| at(n, ".ends without .subckt"))?;
                    deck.subckts.push(s);
                }
                ".model" => {
                    let m = parse_model(&line, n)?;
                    for (_, e) in &m.params {
                        check_functions(e, if open.is_some() { &sub_funcs } else { &file_funcs }, n)?;
                    }
                    match &mut open {
                        Some(s) => s.models.push(m),
                        None => deck.models.push(m),
                    }
                }
                ".param" | ".params" => {
                    let ts = tokens(&line, n)?;
                    let ps = pairs(&ts[1..], n)?;
                    for (_, e) in &ps {
                        check_functions(e, if open.is_some() { &sub_funcs } else { &file_funcs }, n)?;
                    }
                    match &mut open {
                        Some(s) => s.params.extend(ps),
                        None => deck.params.extend(ps),
                    }
                }
                ".func" => {
                    let f = parse_func(&line, n)?;
                    let known = if open.is_some() { &mut sub_funcs } else { &mut file_funcs };
                    let body = expr::inline(&f.1.body, known).map_err(|e| at(n, e.message))?;
                    check_functions(&body, known, n)?;
                    known.insert(f.0.clone(), Func { params: f.1.params.clone(), body });
                    match &mut open {
                        Some(s) => s.funcs.push(f),
                        None => deck.funcs.push(f),
                    }
                }
                other => {
                    return Err(at(
                        n,
                        format!(
                            "{other} is not allowed in an imported model (only .subckt, .ends, .model, .param, .func)"
                        ),
                    ));
                }
            }
            continue;
        }
        let Some(s) = &mut open else {
            return Err(at(n, "elements belong inside a .subckt"));
        };
        elements += 1;
        if elements > MAX_ELEMENTS {
            return Err(at(n, format!("at most {MAX_ELEMENTS} elements")));
        }
        let el = parse_element(&line, n, &model_names)?;
        for e in element_exprs(&el) {
            check_functions(e, &sub_funcs, n)?;
        }
        s.elements.push(el);
    }
    if open.is_some() {
        return Err(ImportError { line: None, message: "a .subckt is not closed with .ends".into() });
    }
    if deck.subckts.is_empty() {
        return Err(ImportError { line: None, message: "no .subckt in the model".into() });
    }
    let mut names = BTreeSet::new();
    for s in &deck.subckts {
        if !names.insert(s.name.clone()) {
            return Err(ImportError { line: None, message: format!(".subckt {} is defined twice", s.name) });
        }
    }
    Ok(deck)
}

/// Any name, node or current: only functions and their arguments are checked here.
struct Permissive;

impl expr::Scope for Permissive {
    fn var(&self, _: &str) -> Option<expr::Ty> {
        Some(expr::Ty::Free)
    }
    fn node(&self, _: &str) -> bool {
        true
    }
    fn current(&self, _: &str) -> bool {
        true
    }
    fn time(&self) -> bool {
        true
    }
}

/// Every expression calls only allow-listed functions (user functions inlined first).
fn check_functions(e: &Expr, funcs: &BTreeMap<String, Func>, n: usize) -> Result<(), ImportError> {
    let e = expr::inline(e, funcs).map_err(|x| at(n, x.message))?;
    let mut bad = None;
    expr::walk(&e, &mut |x| {
        if let Expr::Call(name, _) = x
            && bad.is_none()
            && expr::check(x, Dialect::Spice, &Permissive).is_err()
        {
            bad = Some(name.clone());
        }
    });
    match bad {
        Some(_) => {
            Err(at(n, expr::check(&e, Dialect::Spice, &Permissive).err().map_or_else(String::new, |e| e.message)))
        }
        None => Ok(()),
    }
}

fn parse_subckt_head(line: &str, n: usize) -> Result<Subckt, ImportError> {
    let lower = line.to_ascii_lowercase();
    let (nodes_part, params_part) = match lower.find("params:") {
        Some(k) => (&line[..k], Some(&line[k + 7..])),
        None => (line, None),
    };
    let words: Vec<&str> = nodes_part.split_whitespace().skip(1).collect();
    let (name, mut ports) = match words.split_first() {
        Some((name, ports)) => (name.to_ascii_lowercase(), ports.iter().map(|p| p.to_string()).collect::<Vec<_>>()),
        None => return Err(at(n, ".subckt needs a name")),
    };
    if !ident(&name) {
        return Err(at(n, format!("\"{name}\" is not a sub-circuit name")));
    }
    // Some vendors write defaults without PARAMS: (`.subckt x a b gain=2`).
    let mut defaults_text = params_part.map(str::to_string).unwrap_or_default();
    if let Some(k) = ports.iter().position(|p| p.contains('=')) {
        defaults_text = format!("{} {defaults_text}", ports[k..].join(" "));
        ports.truncate(k);
    }
    for p in &ports {
        if !node_ok(p) || p == "0" {
            return Err(at(n, format!("\"{p}\" is not a port name")));
        }
    }
    let defaults = pairs(&tokens(&defaults_text, n)?, n)?;
    Ok(Subckt {
        name,
        ports,
        defaults,
        params: Vec::new(),
        funcs: Vec::new(),
        models: Vec::new(),
        elements: Vec::new(),
    })
}

fn parse_model(line: &str, n: usize) -> Result<ModelCard, ImportError> {
    let ts = tokens(line, n)?;
    let (Some(T::Word(name)), Some(T::Word(kind))) = (ts.get(1), ts.get(2)) else {
        return Err(at(n, ".model name type (params)"));
    };
    let kind = kind.to_ascii_lowercase();
    if !MODEL_KINDS.contains(&kind.as_str()) {
        return Err(at(n, format!("model type {kind} is not one of {}", MODEL_KINDS.join(", "))));
    }
    if !ident(name) && !name.chars().all(|c| c.is_ascii_alphanumeric() || "_-.".contains(c)) {
        return Err(at(n, format!("\"{name}\" is not a model name")));
    }
    Ok(ModelCard { name: name.to_ascii_lowercase(), kind, params: pairs(&ts[3..], n)? })
}

fn parse_func(line: &str, n: usize) -> Result<(String, Func), ImportError> {
    let rest = line.trim_start()[5..].trim();
    let close = rest.find(')').ok_or_else(|| at(n, ".func name(args) {expression}"))?;
    let (name, params) = expr::parse_head(&rest[..=close]).map_err(|e| at(n, e.message))?;
    let body = rest[close + 1..].trim();
    let body = body.strip_prefix('=').map_or(body, str::trim);
    let body = body
        .strip_prefix('{')
        .and_then(|b| b.strip_suffix('}'))
        .or_else(|| body.strip_prefix('\'').and_then(|b| b.strip_suffix('\'')))
        .unwrap_or(body);
    let body = expr::parse(body, Dialect::Spice).map_err(|e| at(n, e.message))?;
    let params = params.iter().map(|p| p.to_ascii_lowercase()).collect();
    Ok((name.to_ascii_lowercase(), Func { params, body }))
}

fn element_name(first: &str, n: usize) -> Result<(char, String), ImportError> {
    let kind = first.chars().next().map(|c| c.to_ascii_uppercase()).unwrap_or(' ');
    if !"RCLDQMJKEFGHBVIX".contains(kind) {
        return Err(at(n, format!("element {first}: only R C L D Q M J K E F G H B V I X are allowed")));
    }
    if !node_ok(first) {
        return Err(at(n, format!("\"{first}\" is not an element name")));
    }
    Ok((kind, first.to_ascii_lowercase()))
}

fn nodes2<'a>(line: &'a str, name: &str, n: usize) -> Result<([String; 2], &'a str), ImportError> {
    let (w, rest) = head(line, 3).ok_or_else(|| at(n, format!("{name}: needs two nodes")))?;
    for x in &w[1..] {
        if !node_ok(x) {
            return Err(at(n, format!("\"{x}\" is not a node name")));
        }
    }
    Ok(([w[1].to_string(), w[2].to_string()], rest))
}

fn parse_element(line: &str, n: usize, models: &BTreeSet<String>) -> Result<Element, ImportError> {
    let first = line.split_whitespace().next().unwrap_or("");
    let (kind, name) = element_name(first, n)?;
    match kind {
        'B' => {
            let (nodes, rest) = nodes2(line, &name, n)?;
            let volt = rest.trim_start().to_ascii_lowercase().starts_with('v');
            let e = tail_expr(rest, if volt { &["v"] } else { &["i"] }, n)?;
            Ok(Element::Behavioral { name, nodes, volt, expr: e })
        }
        'E' | 'G' | 'F' | 'H' => {
            let (nodes, rest) = nodes2(line, &name, n)?;
            let low = rest.trim_start().to_ascii_lowercase();
            for refused in ["table", "laplace", "freq", "chebyshev"] {
                if low.starts_with(refused) {
                    return Err(at(n, format!("{name}: {} sources are not supported", refused.to_ascii_uppercase())));
                }
            }
            if matches!(kind, 'E' | 'G')
                && (low.starts_with("value") || low.starts_with("vol") || low.starts_with("cur"))
            {
                let e = tail_expr(rest, &["value", "vol", "cur"], n)?;
                return Ok(Element::Behavioral { name, nodes, volt: kind == 'E', expr: e });
            }
            let ts = tokens(rest, n)?;
            let per = if matches!(kind, 'E' | 'G') { 2 } else { 1 };
            let (poly, body) = match ts.as_slice() {
                [T::Word(p), T::Open, T::Word(d), T::Close, body @ ..] if p.eq_ignore_ascii_case("poly") => {
                    let d: usize = d.parse().map_err(|_| at(n, "POLY(n) needs a whole number"))?;
                    if !(1..=MAX_POLY_DIMS).contains(&d) {
                        return Err(at(n, format!("POLY dimension must be 1 to {MAX_POLY_DIMS}")));
                    }
                    (Some(d), body)
                }
                _ => (None, ts.as_slice()),
            };
            let body: Vec<&T> = body.iter().filter(|t| !matches!(t, T::Comma | T::Open | T::Close)).collect();
            let nctrl = per * poly.unwrap_or(1);
            if body.len() < nctrl + 1 {
                return Err(at(n, format!("{name}: expected {nctrl} controls and the coefficients")));
            }
            let mut ctrl = Vec::new();
            for t in &body[..nctrl] {
                match t {
                    T::Word(w) if node_ok(w) => ctrl.push(w.clone()),
                    _ => return Err(at(n, format!("{name}: expected a control node or source"))),
                }
            }
            let coeffs: Vec<Expr> = body[nctrl..].iter().map(|t| value(t, n)).collect::<Result<_, _>>()?;
            if poly.is_none() && coeffs.len() != 1 {
                return Err(at(n, format!("{name}: a linear source has one gain")));
            }
            Ok(Element::Controlled { kind, name, nodes, ctrl, coeffs, poly })
        }
        'V' | 'I' => {
            let (nodes, rest) = nodes2(line, &name, n)?;
            let ts = tokens(rest, n)?;
            let ts: Vec<&T> = ts.iter().filter(|t| !matches!(t, T::Word(w) if w.eq_ignore_ascii_case("dc"))).collect();
            let dc = match ts.as_slice() {
                [] => Expr::Num(0.0, None),
                [v] => value(v, n)?,
                _ => return Err(at(n, format!("{name}: only DC sources are allowed inside a model"))),
            };
            Ok(Element::Source { kind, name, nodes, dc })
        }
        'R' | 'C' | 'L' => {
            let (nodes, rest) = nodes2(line, &name, n)?;
            let ts = tokens(rest, n)?;
            let key = match kind {
                'R' => "r",
                'C' => "c",
                _ => "l",
            };
            let (value_t, extra_t) = match ts.as_slice() {
                [T::Word(k), T::Eq, v, extra @ ..]
                    if k.eq_ignore_ascii_case(key) || k.eq_ignore_ascii_case("value") =>
                {
                    (v, extra)
                }
                [v, extra @ ..] if !matches!(v, T::Eq) => (v, extra),
                _ => return Err(at(n, format!("{name}: needs a value"))),
            };
            let extra = pairs(extra_t, n)?;
            for (k, _) in &extra {
                if !extra_keys(kind).contains(&k.as_str()) {
                    return Err(at(n, format!("{name}: {k}= is not supported")));
                }
            }
            Ok(Element::Passive { kind, name, nodes, value: value(value_t, n)?, extra })
        }
        'D' | 'Q' | 'M' | 'J' => {
            let ts = tokens(line, n)?;
            let words: Vec<String> = ts
                .iter()
                .take_while(|t| matches!(t, T::Word(_)))
                .map(|t| match t {
                    T::Word(w) => w.clone(),
                    _ => unreachable!(),
                })
                .collect();
            // Nodes, then the model; then an optional area and key=value pairs.
            let fixed = match kind {
                'D' => 2,
                'J' => 3,
                'M' => 4,
                _ => 3,
            };
            let model_at = if kind == 'Q'
                && words.get(4).is_some_and(|w| models.contains(&w.to_ascii_lowercase()))
                && !words.get(3).is_some_and(|w| models.contains(&w.to_ascii_lowercase()))
            {
                4
            } else {
                fixed + 1
            };
            let model = words
                .get(model_at)
                .ok_or_else(|| at(n, format!("{name}: needs {} nodes and a model", model_at - 1)))?;
            let model = model.to_ascii_lowercase();
            if !models.contains(&model) {
                return Err(at(n, format!("{name}: no .model {model} in the file")));
            }
            let nodes: Vec<String> = words[1..model_at].to_vec();
            for x in &nodes {
                if !node_ok(x) {
                    return Err(at(n, format!("\"{x}\" is not a node name")));
                }
            }
            // What follows the model: words up to the first `key=`.
            let mut rest: Vec<T> = ts[model_at + 1..].to_vec();
            let mut area = None;
            let mut flags = Vec::new();
            while let Some(T::Word(w)) = rest.first().cloned() {
                if matches!(rest.get(1), Some(T::Eq)) {
                    break;
                }
                if w.eq_ignore_ascii_case("off") {
                    flags.push(w);
                } else if area.is_none() {
                    area = Some(value(&T::Word(w), n)?);
                } else {
                    return Err(at(n, format!("{name}: unexpected \"{w}\"")));
                }
                rest.remove(0);
            }
            if !flags.is_empty() {
                return Err(at(n, format!("{name}: OFF is not supported")));
            }
            let extra = pairs(&rest, n)?;
            for (k, _) in &extra {
                if !extra_keys(kind).contains(&k.as_str()) {
                    return Err(at(n, format!("{name}: {k}= is not supported")));
                }
            }
            Ok(Element::Device { kind, name, nodes, model, area, extra })
        }
        'K' => {
            let ts = tokens(line, n)?;
            match ts.as_slice() {
                [_, T::Word(a), T::Word(b), k] => Ok(Element::Coupling {
                    name,
                    a: a.to_ascii_lowercase(),
                    b: b.to_ascii_lowercase(),
                    k: value(k, n)?,
                }),
                _ => Err(at(n, format!("{name}: K L1 L2 coupling"))),
            }
        }
        'X' => {
            let lower = line.to_ascii_lowercase();
            let (main, params_text) = match lower.find("params:") {
                Some(k) => (&line[..k], &line[k + 7..]),
                None => (line, ""),
            };
            let ts = tokens(main, n)?;
            let eq = ts.iter().position(|t| matches!(t, T::Eq));
            let (words_end, more) = match eq {
                Some(k) if k >= 1 => (k - 1, &ts[k - 1..]),
                _ => (ts.len(), &[][..]),
            };
            let mut words = Vec::new();
            for t in &ts[1..words_end] {
                match t {
                    T::Word(w) => words.push(w.clone()),
                    _ => return Err(at(n, format!("{name}: nodes, then the sub-circuit name"))),
                }
            }
            let sub =
                words.pop().ok_or_else(|| at(n, format!("{name}: needs a sub-circuit name")))?.to_ascii_lowercase();
            for x in &words {
                if !node_ok(x) {
                    return Err(at(n, format!("\"{x}\" is not a node name")));
                }
            }
            let mut params = pairs(more, n)?;
            params.extend(pairs(&tokens(params_text, n)?, n)?);
            Ok(Element::Call { name, nodes: words, sub, params })
        }
        _ => unreachable!("checked by element_name"),
    }
}

// ---------------------------------------------------------------- evaluation and emission

/// Values of params, layered: file, then sub-circuit defaults (overridden), then local `.param`s.
struct Env {
    values: BTreeMap<String, f64>,
    funcs: BTreeMap<String, Func>,
}

impl Env {
    fn eval(&self, e: &Expr, what: &str) -> Result<f64, String> {
        let e = expr::inline(e, &self.funcs).map_err(|e| format!("{what}: {e}"))?;
        let r = expr::eval(&e, &self.values).map_err(|e| format!("{what}: {e}"))?;
        if !r.value.is_finite() {
            return Err(format!("{what} is not a finite number"));
        }
        // Six significant digits for what only libm computes, so the netlist is identical everywhere.
        Ok(if r.inexact && r.value != 0.0 { format!("{:.5e}", r.value).parse().expect("re-parses") } else { r.value })
    }
}

fn num(x: f64) -> String {
    spice_number(x)
}

/// What one import compiles to: model cards and `.subckt` blocks, the main one named `name`.
#[derive(Clone, Debug, PartialEq)]
pub struct Emitted {
    pub lines: Vec<String>,
    /// Primitive devices one instance simulates (helper sub-circuits counted per call).
    pub leaf_devices: u64,
}

/// A helper sub-circuit: its index and its param values (as bits, exact).
type HelperKey = (usize, Vec<(String, u64)>);

struct Emitter<'a> {
    deck: &'a Deck,
    prefix: String,
    lines: Vec<String>,
    models: BTreeMap<(usize, String), String>,
    /// Emitted helper sub-circuits -> name and leaf count.
    helpers: BTreeMap<HelperKey, (String, u64)>,
    depth: usize,
}

/// Compile the model at `params` (component param values by name; those that match a `PARAMS:`
/// default of the main sub-circuit override it), naming the main sub-circuit `name` with its
/// ports in `pins` order (component pins, lowercased).
pub fn compile(m: &SpiceModel, pins: &[String], params: &BTreeMap<String, f64>, name: &str) -> Result<Emitted, String> {
    let deck = parse(&m.text).map_err(|e| e.to_string())?;
    let main = deck
        .subckts
        .iter()
        .position(|s| s.name == m.subckt.to_ascii_lowercase())
        .ok_or_else(|| format!("no .subckt {} in the model", m.subckt))?;
    let mut e = Emitter {
        deck: &deck,
        prefix: name.to_string(),
        lines: Vec::new(),
        models: BTreeMap::new(),
        helpers: BTreeMap::new(),
        depth: 0,
    };
    let file = e.file_env()?;
    let lower_params: BTreeMap<String, f64> = params.iter().map(|(k, v)| (k.to_ascii_lowercase(), *v)).collect();
    let ports = port_order(&deck.subckts[main], m, pins)?;
    let leaf = e.subckt(main, name, &ports, &file, &lower_params)?;
    Ok(Emitted { lines: e.lines, leaf_devices: leaf })
}

/// The main sub-circuit's ports in component pin order.
fn port_order(s: &Subckt, m: &SpiceModel, pins: &[String]) -> Result<Vec<String>, String> {
    let mut used = BTreeSet::new();
    let mut out = Vec::new();
    for pin in pins {
        let port = m
            .pins
            .iter()
            .find(|(p, _)| p.eq_ignore_ascii_case(pin))
            .map(|(_, port)| port)
            .ok_or_else(|| format!("pin {pin} is mapped to no port of {}", s.name))?;
        let k = s
            .ports
            .iter()
            .position(|p| p.eq_ignore_ascii_case(port))
            .ok_or_else(|| format!("{} has no port {port}", s.name))?;
        if !used.insert(k) {
            return Err(format!("port {port} is mapped twice"));
        }
        out.push(s.ports[k].clone());
    }
    if used.len() != s.ports.len() {
        let missing: Vec<&str> =
            s.ports.iter().enumerate().filter(|(k, _)| !used.contains(k)).map(|(_, p)| p.as_str()).collect();
        return Err(format!("ports {} of {} are mapped to no pin", missing.join(", "), s.name));
    }
    Ok(out)
}

impl Emitter<'_> {
    fn file_env(&self) -> Result<Env, String> {
        let mut env = Env { values: BTreeMap::new(), funcs: BTreeMap::new() };
        for (n, f) in &self.deck.funcs {
            let body = expr::inline(&f.body, &env.funcs).map_err(|e| format!(".func {n}: {e}"))?;
            env.funcs.insert(n.clone(), Func { params: f.params.clone(), body });
        }
        for (k, v) in &self.deck.params {
            let x = env.eval(v, &format!(".param {k}"))?;
            env.values.insert(k.clone(), x);
        }
        Ok(env)
    }

    /// Emit sub-circuit `idx` as `name` with `ports` (its own port names in emission order).
    /// Returns the leaf devices one call of it simulates.
    fn subckt(
        &mut self,
        idx: usize,
        name: &str,
        ports: &[String],
        outer: &Env,
        given: &BTreeMap<String, f64>,
    ) -> Result<u64, String> {
        self.depth += 1;
        if self.depth > 16 {
            return Err("sub-circuits nest more than 16 deep (or call themselves)".into());
        }
        let s = &self.deck.subckts[idx];
        let mut env = Env { values: outer.values.clone(), funcs: outer.funcs.clone() };
        for (k, v) in &s.defaults {
            let x = match given.get(k) {
                Some(x) => *x,
                None => env.eval(v, &format!("{} param {k}", s.name))?,
            };
            env.values.insert(k.clone(), x);
        }
        for (n, f) in &s.funcs {
            let body = expr::inline(&f.body, &env.funcs).map_err(|e| format!(".func {n}: {e}"))?;
            env.funcs.insert(n.clone(), Func { params: f.params.clone(), body });
        }
        for (k, v) in &s.params {
            let x = env.eval(v, &format!(".param {k}"))?;
            env.values.insert(k.clone(), x);
        }
        // Nodes: ports by position, ground, then internal nodes in order of appearance.
        let mut nodes: BTreeMap<String, String> = BTreeMap::new();
        for (k, p) in s.ports.iter().enumerate() {
            let at =
                ports.iter().position(|q| q.eq_ignore_ascii_case(p)).ok_or_else(|| format!("port {p} has no place"))?;
            let _ = k;
            nodes.insert(p.to_ascii_lowercase(), format!("p{at}"));
        }
        let port_names: Vec<String> = (0..ports.len()).map(|k| format!("p{k}")).collect();
        let mut node = |raw: &str| -> String {
            let l = raw.to_ascii_lowercase();
            if l == "0" || l == "gnd" {
                return "0".into();
            }
            let next = nodes.len();
            nodes.entry(l).or_insert_with(|| format!("n{next}")).clone()
        };
        // Element and source names: kind letter + index.
        let mut renamed: BTreeMap<String, String> = BTreeMap::new();
        for (k, el) in s.elements.iter().enumerate() {
            let (kind, name) = element_kind_name(el);
            renamed.insert(name.to_string(), format!("{}{}", kind.to_ascii_lowercase(), k + 1));
        }
        let mut body = Vec::new();
        let mut leaf = 0u64;
        for el in &s.elements {
            let (kind, ename) = element_kind_name(el);
            let me = renamed[ename].clone();
            let what = format!("{} {ename}", s.name);
            match el {
                Element::Passive { nodes: ns, value, extra, .. } => {
                    let v = env.eval(value, &what)?;
                    let mut line = format!("{me} {} {} {}", node(&ns[0]), node(&ns[1]), num(v));
                    for (k, x) in extra {
                        line.push_str(&format!(" {k}={}", num(env.eval(x, &what)?)));
                    }
                    body.push(line);
                    leaf += 1;
                }
                Element::Device { nodes: ns, model, area, extra, .. } => {
                    let mname = self.model(idx, model, &env)?;
                    let ns: Vec<String> = ns.iter().map(|x| node(x)).collect();
                    let mut line = format!("{me} {} {mname}", ns.join(" "));
                    if let Some(a) = area {
                        line.push_str(&format!(" {}", num(env.eval(a, &what)?)));
                    }
                    for (k, x) in extra {
                        line.push_str(&format!(" {k}={}", num(env.eval(x, &what)?)));
                    }
                    body.push(line);
                    leaf += 1;
                }
                Element::Coupling { a, b, k, .. } => {
                    let la = renamed
                        .get(a)
                        .filter(|r| r.starts_with('l'))
                        .ok_or_else(|| format!("{what}: no inductor {a}"))?;
                    let lb = renamed
                        .get(b)
                        .filter(|r| r.starts_with('l'))
                        .ok_or_else(|| format!("{what}: no inductor {b}"))?;
                    body.push(format!("{me} {la} {lb} {}", num(env.eval(k, &what)?)));
                    leaf += 1;
                }
                Element::Source { nodes: ns, dc, .. } => {
                    body.push(format!("{me} {} {} DC {}", node(&ns[0]), node(&ns[1]), num(env.eval(dc, &what)?)));
                    leaf += 1;
                }
                Element::Controlled { kind, nodes: ns, ctrl, coeffs, poly, .. } => {
                    let cs: Vec<f64> = coeffs.iter().map(|c| env.eval(c, &what)).collect::<Result<_, _>>()?;
                    let source = |v: &str| -> Result<String, String> {
                        renamed
                            .get(&v.to_ascii_lowercase())
                            .filter(|r| r.starts_with('v'))
                            .cloned()
                            .ok_or_else(|| format!("{what}: no voltage source {v}"))
                    };
                    let (a, b) = (node(&ns[0]), node(&ns[1]));
                    match poly {
                        None => {
                            let ctl = if matches!(kind, 'E' | 'G') {
                                format!("{} {}", node(&ctrl[0]), node(&ctrl[1]))
                            } else {
                                source(&ctrl[0])?
                            };
                            body.push(format!("{me} {a} {b} {ctl} {}", num(cs[0])));
                        }
                        Some(d) => {
                            let xs: Vec<String> = if matches!(kind, 'E' | 'G') {
                                (0..*d)
                                    .map(|i| format!("v({},{})", node(&ctrl[2 * i]), node(&ctrl[2 * i + 1])))
                                    .collect()
                            } else {
                                (0..*d)
                                    .map(|i| source(&ctrl[i]).map(|v| format!("i({v})")))
                                    .collect::<Result<_, _>>()?
                            };
                            let volt = matches!(kind, 'E' | 'H');
                            body.push(format!(
                                "b{me} {a} {b} {}={}",
                                if volt { "V" } else { "I" },
                                poly_text(&xs, &cs)
                            ));
                        }
                    }
                    leaf += 1;
                }
                Element::Behavioral { nodes: ns, volt, expr: e, .. } => {
                    let e = expr::inline(e, &env.funcs).map_err(|x| format!("{what}: {x}"))?;
                    let (a, b) = (node(&ns[0]), node(&ns[1]));
                    let names = ImportNames { env: &env, renamed: &renamed, nodes: std::cell::RefCell::new(&mut node) };
                    let text = expr::emit(&e, &names).map_err(|x| format!("{what}: {x}"))?;
                    let prefix = if kind == 'B' { String::new() } else { "b".into() };
                    body.push(format!("{prefix}{me} {a} {b} {}={text}", if *volt { "V" } else { "I" }));
                    leaf += 1;
                }
                Element::Call { nodes: ns, sub, params, .. } => {
                    let k = self
                        .deck
                        .subckts
                        .iter()
                        .position(|x| &x.name == sub)
                        .ok_or_else(|| format!("{what}: no .subckt {sub} in the file"))?;
                    let callee = &self.deck.subckts[k];
                    if ns.len() != callee.ports.len() {
                        return Err(format!(
                            "{what}: {sub} has {} ports, the call gives {}",
                            callee.ports.len(),
                            ns.len()
                        ));
                    }
                    let mut given = BTreeMap::new();
                    for (pk, pv) in params {
                        if !callee.defaults.iter().any(|(d, _)| d == pk) {
                            return Err(format!("{what}: {sub} has no param {pk}"));
                        }
                        given.insert(pk.clone(), env.eval(pv, &what)?);
                    }
                    let key = (k, given.iter().map(|(a, b)| (a.clone(), b.to_bits())).collect::<Vec<_>>());
                    let (hname, hleaf) = match self.helpers.get(&key) {
                        Some(h) => h.clone(),
                        None => {
                            let hname = format!("{}__{}", self.prefix, self.helpers.len() + 1);
                            let file = self.file_env()?;
                            let hl = self.subckt(k, &hname, &callee.ports, &file, &given)?;
                            self.helpers.insert(key, (hname.clone(), hl));
                            (hname, hl)
                        }
                    };
                    let ns: Vec<String> = ns.iter().map(|x| node(x)).collect();
                    body.push(format!("{me} {} {hname}", ns.join(" ")));
                    leaf += hleaf;
                }
            }
            let _ = kind;
        }
        self.lines.push(format!(".subckt {name} {}", port_names.join(" ")));
        self.lines.extend(body);
        self.lines.push(format!(".ends {name}"));
        self.depth -= 1;
        Ok(leaf)
    }

    /// A model card, emitted once per (scope, name) at the top level under a generated name.
    fn model(&mut self, idx: usize, name: &str, env: &Env) -> Result<String, String> {
        let s = &self.deck.subckts[idx];
        let (scope, card) = match s.models.iter().find(|m| m.name == name) {
            Some(m) => (idx + 1, m),
            None => (0, self.deck.models.iter().find(|m| m.name == name).ok_or_else(|| format!("no .model {name}"))?),
        };
        if let Some(n) = self.models.get(&(scope, name.to_string())) {
            return Ok(n.clone());
        }
        let generated = format!("{}_m{}", self.prefix, self.models.len() + 1);
        let mut line = format!(".model {generated} {}(", card.kind);
        let mut first = true;
        for (k, v) in &card.params {
            if !first {
                line.push(' ');
            }
            first = false;
            line.push_str(&format!("{k}={}", num(env.eval(v, &format!(".model {name} {k}"))?)));
        }
        line.push(')');
        self.lines.push(line);
        self.models.insert((scope, name.to_string()), generated.clone());
        Ok(generated)
    }
}

fn element_exprs(el: &Element) -> Vec<&Expr> {
    match el {
        Element::Passive { value, extra, .. } => std::iter::once(value).chain(extra.iter().map(|(_, e)| e)).collect(),
        Element::Device { area, extra, .. } => area.iter().chain(extra.iter().map(|(_, e)| e)).collect(),
        Element::Coupling { k, .. } => vec![k],
        Element::Controlled { coeffs, .. } => coeffs.iter().collect(),
        Element::Behavioral { expr, .. } => vec![expr],
        Element::Source { dc, .. } => vec![dc],
        Element::Call { params, .. } => params.iter().map(|(_, e)| e).collect(),
    }
}

fn element_kind_name(el: &Element) -> (char, &str) {
    match el {
        Element::Passive { kind, name, .. }
        | Element::Device { kind, name, .. }
        | Element::Controlled { kind, name, .. }
        | Element::Source { kind, name, .. } => (*kind, name),
        Element::Coupling { name, .. } => ('K', name),
        Element::Behavioral { name, .. } => (name.chars().next().map(|c| c.to_ascii_uppercase()).unwrap_or('B'), name),
        Element::Call { name, .. } => ('X', name),
    }
}

/// SPICE2 POLY: p0 + Σ p_i x_i + Σ_{i≤j} p_ij x_i x_j + …, terms by degree, index tuples in
/// lexicographic order.
fn poly_text(xs: &[String], cs: &[f64]) -> String {
    let mut terms = Vec::new();
    let mut k = 0;
    let mut degree = 0;
    'outer: while k < cs.len() {
        let tuples = monomials(xs.len(), degree);
        for t in tuples {
            if k >= cs.len() {
                break 'outer;
            }
            let c = cs[k];
            k += 1;
            if c == 0.0 {
                continue;
            }
            let c = if c < 0.0 { format!("({})", num(c)) } else { num(c) };
            if t.is_empty() {
                terms.push(c);
            } else {
                let f: Vec<&str> = t.iter().map(|i| xs[*i].as_str()).collect();
                terms.push(format!("({c}*{})", f.join("*")));
            }
        }
        degree += 1;
    }
    if terms.is_empty() { "0".into() } else { format!("({})", terms.join("+")) }
}

/// Non-decreasing index tuples of length `degree` over `n` variables, lexicographic.
fn monomials(n: usize, degree: usize) -> Vec<Vec<usize>> {
    if degree == 0 {
        return vec![Vec::new()];
    }
    let mut out = Vec::new();
    for t in monomials(n, degree - 1) {
        let start = t.last().copied().unwrap_or(0);
        for i in start..n {
            let mut u = t.clone();
            u.push(i);
            out.push(u);
        }
    }
    // Lexicographic order of the tuples.
    out.sort();
    out
}

struct ImportNames<'a> {
    env: &'a Env,
    renamed: &'a BTreeMap<String, String>,
    nodes: std::cell::RefCell<&'a mut dyn FnMut(&str) -> String>,
}

impl Names for ImportNames<'_> {
    fn var(&self, name: &str) -> Option<String> {
        self.env.values.get(name).map(|v| if *v < 0.0 { format!("({})", num(*v)) } else { num(*v) })
    }
    fn node(&self, name: &str) -> Option<String> {
        Some((self.nodes.borrow_mut())(name))
    }
    fn current(&self, name: &str) -> Option<String> {
        self.renamed.get(&name.to_ascii_lowercase()).filter(|r| r.starts_with('v')).cloned()
    }
}

/// Port groups of a sub-circuit joined by a DC path inside it, and whether each also reaches
/// ground: the pins' `dc_group`s.
pub fn conducting(m: &SpiceModel) -> Result<Vec<(Vec<String>, bool)>, String> {
    let deck = parse(&m.text).map_err(|e| e.to_string())?;
    let main = deck
        .subckts
        .iter()
        .position(|s| s.name == m.subckt.to_ascii_lowercase())
        .ok_or_else(|| format!("no .subckt {} in the model", m.subckt))?;
    let mut memo = BTreeMap::new();
    let groups = port_groups(&deck, main, &mut memo, 0)?;
    let s = &deck.subckts[main];
    Ok(groups
        .into_iter()
        .map(|(ports, gnd)| {
            let pins = ports
                .iter()
                .filter_map(|k| {
                    m.pins.iter().find(|(_, port)| port.eq_ignore_ascii_case(&s.ports[*k])).map(|(p, _)| p.clone())
                })
                .collect();
            (pins, gnd)
        })
        .collect())
}

/// For sub-circuit `idx`: groups of port indices joined by DC paths, and whether each reaches ground.
fn port_groups(
    deck: &Deck,
    idx: usize,
    memo: &mut BTreeMap<usize, Vec<(Vec<usize>, bool)>>,
    depth: usize,
) -> Result<Vec<(Vec<usize>, bool)>, String> {
    if let Some(g) = memo.get(&idx) {
        return Ok(g.clone());
    }
    if depth > 16 {
        return Err("sub-circuits nest more than 16 deep (or call themselves)".into());
    }
    let s = &deck.subckts[idx];
    let mut index: BTreeMap<String, usize> = BTreeMap::new();
    let mut id = |n: &str| -> usize {
        let l = n.to_ascii_lowercase();
        let l = if l == "gnd" { "0".to_string() } else { l };
        let next = index.len();
        *index.entry(l).or_insert(next)
    };
    let gnd = id("0");
    let ports: Vec<usize> = s.ports.iter().map(|p| id(p)).collect();
    let mut joins: Vec<(usize, usize)> = Vec::new();
    for el in &s.elements {
        match el {
            Element::Passive { kind, nodes, .. } if *kind != 'C' => joins.push((id(&nodes[0]), id(&nodes[1]))),
            Element::Source { kind: 'V', nodes, .. } => joins.push((id(&nodes[0]), id(&nodes[1]))),
            Element::Controlled { kind: 'E' | 'H', nodes, .. } => joins.push((id(&nodes[0]), id(&nodes[1]))),
            Element::Behavioral { volt: true, nodes, .. } => joins.push((id(&nodes[0]), id(&nodes[1]))),
            Element::Device { kind, nodes, .. } => {
                // Junctions conduct; a MOSFET's gate does not.
                let ns: Vec<usize> =
                    nodes.iter().enumerate().filter(|(k, _)| !(*kind == 'M' && *k == 1)).map(|(_, n)| id(n)).collect();
                for w in ns.windows(2) {
                    joins.push((w[0], w[1]));
                }
            }
            Element::Call { nodes, sub, .. } => {
                let k = deck.subckts.iter().position(|x| &x.name == sub).ok_or_else(|| format!("no .subckt {sub}"))?;
                let inner = port_groups(deck, k, memo, depth + 1)?;
                for (group, to_ground) in inner {
                    let ns: Vec<usize> = group.iter().filter_map(|p| nodes.get(*p)).map(|n| id(n)).collect();
                    for w in ns.windows(2) {
                        joins.push((w[0], w[1]));
                    }
                    if to_ground && let Some(first) = ns.first() {
                        joins.push((*first, gnd));
                    }
                }
            }
            _ => {}
        }
    }
    let mut parent: Vec<usize> = (0..index.len()).collect();
    fn find(p: &mut [usize], x: usize) -> usize {
        let mut r = x;
        while p[r] != r {
            r = p[r];
        }
        let mut y = x;
        while p[y] != r {
            let next = p[y];
            p[y] = r;
            y = next;
        }
        r
    }
    for (a, b) in joins {
        let (ra, rb) = (find(&mut parent, a), find(&mut parent, b));
        if ra != rb {
            parent[ra.max(rb)] = ra.min(rb);
        }
    }
    let groot = find(&mut parent, gnd);
    let mut by_root: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    for (k, p) in ports.iter().enumerate() {
        by_root.entry(find(&mut parent, *p)).or_default().push(k);
    }
    let out: Vec<(Vec<usize>, bool)> = by_root.into_iter().map(|(r, ks)| (ks, r == groot)).collect();
    memo.insert(idx, out.clone());
    Ok(out)
}

/// Names of the main sub-circuit's `PARAMS:` defaults, which component params may set.
pub fn defaults(m: &SpiceModel) -> Result<Vec<String>, String> {
    let deck = parse(&m.text).map_err(|e| e.to_string())?;
    let s = deck
        .subckts
        .iter()
        .find(|s| s.name == m.subckt.to_ascii_lowercase())
        .ok_or_else(|| format!("no .subckt {} in the model", m.subckt))?;
    Ok(s.defaults.iter().map(|(k, _)| k.clone()).collect())
}

/// The first word of every emitted line is one of these: nothing else reaches the simulator.
pub fn allowed_line(line: &str) -> bool {
    let first = line.split_whitespace().next().unwrap_or("");
    let lower = first.to_ascii_lowercase();
    matches!(lower.as_str(), ".subckt" | ".ends" | ".model")
        || (lower.len() > 1
            && "rcldqmjkefghbvix".contains(&lower[..1])
            && lower[1..].chars().all(|c| c.is_ascii_alphanumeric() || c == '_'))
}

#[cfg(test)]
mod tests {
    use super::*;

    const OPAMP: &str = "\
* A vendor-style macromodel
.param gbw=1meg
.func half(x) {x/2}
.subckt demo_op inp inm vcc vee out params: aol=100k
rin inp inm 1meg
e1 n1 0 inp inm {aol}
r1 n1 n2 {aol/(2*3.14159265*gbw)*0.001}
c1 n2 0 1u
gpoly 0 n3 poly(2) n2 0 inp 0 0 1m 2m 0.5m
rpoly n3 0 1k
vsense n4 out 0
bout n4 0 v=min(max(v(n2), v(vee)+0.02), v(vcc)-half(3))
d1 out vcc dclamp
d2 vee out dclamp
xbias vcc vee bias params: i=0.5m
.model dclamp d(is=1e-14 n=1)
.ends demo_op
.subckt bias a b i=1m
i1 a b {i}
.ends
.end
";

    fn model() -> SpiceModel {
        SpiceModel {
            text: OPAMP.into(),
            subckt: "demo_op".into(),
            pins: [("INP", "inp"), ("INM", "inm"), ("VCC", "vcc"), ("VEE", "vee"), ("OUT", "out")]
                .into_iter()
                .map(|(a, b)| (a.to_string(), b.to_string()))
                .collect(),
        }
    }

    #[test]
    fn parses_and_reemits_from_the_tree() {
        let pins: Vec<String> = ["inp", "inm", "vcc", "vee", "out"].iter().map(|s| s.to_string()).collect();
        let out = compile(&model(), &pins, &BTreeMap::from([("aol".to_string(), 2e5)]), "lib_x").unwrap();
        let text = out.lines.join("\n");
        assert!(out.lines.iter().all(|l| allowed_line(l)), "{text}");
        assert!(text.contains(".subckt lib_x p0 p1 p2 p3 p4"), "{text}");
        assert!(text.contains("e2 n5 0 p0 p1 2e5"), "{text}");
        assert!(text.contains("bg5 0 n7 I=((1e-3*v(n6,0))+(2e-3*v(p0,0))+(5e-4*v(n6,0)*v(n6,0)))"), "{text}");
        assert!(text.contains("b8 n8 0 V=min(max(v(n6),(v(p3)+2e-2)),(v(p2)-(3e0/2e0)))"), "{text}");
        assert!(text.contains(".model lib_x_m1 d(is=1e-14 n=1e0)"), "{text}");
        assert!(text.contains(".subckt lib_x__1 p0 p1\ni1 p0 p1 DC 5e-4\n.ends lib_x__1"), "{text}");
        assert_eq!(out.leaf_devices, 10 + 1, "{text}");
        let groups = conducting(&model()).unwrap();
        let names = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        // The output, through B's source and the clamp diodes, reaches ground and the rails; the
        // inputs only reach each other (rin): E's controls do not conduct.
        assert_eq!(groups, [(names(&["VCC", "VEE", "OUT"]), true), (names(&["INP", "INM"]), false)]);
    }

    #[test]
    fn refuses_what_is_not_allowed() {
        let bad = |text: &str| parse(text).unwrap_err().to_string();
        assert_eq!(
            bad(".include evil.lib\n.subckt a x\nr1 x 0 1\n.ends"),
            "line 1: .include is not allowed in an imported model (only .subckt, .ends, .model, .param, .func)"
        );
        assert!(bad(".subckt a x\n.control\nshell rm -rf /\n.endc\n.ends").contains(".control is not allowed"));
        assert!(bad(".subckt a x\nv1 x 0 pulse(0 1 0 1n 1n 1u 2u)\n.ends").contains("only DC sources"));
        assert!(bad(".subckt a x\ne1 x 0 laplace {v(x)} = {1/s}\n.ends").contains("LAPLACE"));
        assert!(
            bad(".subckt a x\nr1 x 0 {system(1)}\n.ends").contains("unknown function")
                || bad(".subckt a x\nr1 x 0 {system(1)}\n.ends").contains("system")
        );
        assert!(bad(".subckt a x\nw1 x 0 vx wmod\n.ends").contains("only R C L D Q M J K E F G H B V I X"));
        assert!(bad(&"*".repeat(MAX_BYTES + 1)).contains("at most 256 KB"));
        assert!(parse(".subckt a x\nr1 x 0 1k\n").unwrap_err().to_string().contains("not closed"));
    }

    #[test]
    fn poly_terms_follow_spice2() {
        assert_eq!(monomials(2, 2), [vec![0, 0], vec![0, 1], vec![1, 1]]);
        let xs = ["a".to_string(), "b".to_string()];
        assert_eq!(poly_text(&xs, &[1.0, 2.0, 0.0, 3.0]), "(1e0+(2e0*a)+(3e0*a*a))");
    }
}
