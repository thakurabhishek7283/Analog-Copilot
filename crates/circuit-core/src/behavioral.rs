//! Behavioral components (LLD §17 D3): a black box written as laws, compiled to B-sources,
//! resistors, capacitors and current sources inside its `.subckt`. Four shapes of law:
//!
//! - static relations: `V(out) = clamp(V(ref) + gain*(V(inp) - V(inm)), V(vee) + 0.02, V(vcc) - 1.5)`,
//!   a B voltage source with hard `min`/`max` limits; `I(a, b) = …` a B current source (a
//!   constant one a plain current source);
//! - linear dynamics: `V(out) = tf(x, [b_m … b_0], [a_n … a_0])`, coefficients of s highest
//!   power first, realised as a controllable-canonical state space: a state per pole, each a
//!   B current into a capacitor of 1/ω0 (time scaled by ω0 so every coefficient is near 1), no
//!   `ddt()` and no XSPICE. `V(out) = clamp(tf(…), lo, hi)` also bounds the state behind the
//!   output: its current is clamped between `K·(lo − x)` and `K·(hi − x)`, so the state stops at
//!   the limit instead of winding up past it (in the P5.0 spike an output-only clamp switched a
//!   Schmitt trigger at 4.34 V instead of 2.83 V);
//! - port physics: `R(a, b) = 1Meg`, `C(a, b) = 2p`, `Rout(out) = 50` (behind `V(out)`'s source),
//!   real elements, so loading and supply current are physical;
//! - the envelope: `require V(vcc) - V(vee) in [3, 32]`, `require I(out) < 20m`: checked after a
//!   transient (and by ERC when the pins sit on known rails), and given to the tutor.
//!
//! One-argument forms (`V(out)`, `R(in)`, `I(vcc)`) are relative to ground. `I(pin)` reads the
//! current into a pin through a zero-volt sense source.

use std::collections::{BTreeMap, BTreeSet};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::expr::{self, BinOp, Dialect, Dim, Expr, Names, Scope, Ty};
use crate::registry::PartDef;
use crate::units::{Unit, spice_number};

/// Laws in one definition.
pub const MAX_LAWS: usize = 64;
/// How much faster than the transfer function's own rate a bounded state is held at its limit.
pub const BOUND_GAIN: f64 = 1e6;
/// Highest order of one transfer function.
pub const MAX_ORDER: usize = 8;
/// Internal nodes and elements start with this; pins may not.
pub const RESERVED: &str = "bx_";

#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Behavioral {
    /// One law per entry, as written.
    pub laws: Vec<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Law {
    Volt { pos: String, neg: Option<String>, rhs: Rhs },
    Curr { from: String, to: Option<String>, value: Expr },
    Res { a: String, b: Option<String>, value: Expr },
    Cap { a: String, b: Option<String>, value: Expr },
    Rout { pin: String, value: Expr },
    Require(Require),
}

#[derive(Clone, Debug, PartialEq)]
pub enum Rhs {
    Plain(Expr),
    Tf { x: Expr, num: Vec<Expr>, den: Vec<Expr>, clamp: Option<(Expr, Expr)> },
}

/// `value` must stay within `lo..hi` (either may be absent).
#[derive(Clone, Debug, PartialEq)]
pub struct Require {
    pub text: String,
    pub value: Expr,
    pub lo: Option<Expr>,
    pub hi: Option<Expr>,
}

/// The parsed laws of a definition, in order, each with its 1-based number.
#[derive(Clone, Debug, PartialEq)]
pub struct Laws {
    pub laws: Vec<Law>,
}

struct LawScope<'a> {
    part: &'a PartDef,
}

impl Scope for LawScope<'_> {
    fn var(&self, name: &str) -> Option<Ty> {
        self.part.params.get(name).map(|p| Ty::Dim(Dim::of(p.unit)))
    }
    fn node(&self, name: &str) -> bool {
        name == crate::ir::GND || self.part.pin(name).is_some()
    }
    fn current(&self, name: &str) -> bool {
        self.part.pin(name).is_some()
    }
    fn time(&self) -> bool {
        true
    }
    fn tf(&self) -> bool {
        true
    }
}

/// The byte offset of the law's `=`: the first one not part of `==`, `<=`, `>=` or `!=`.
fn assignment(text: &str) -> Option<usize> {
    let b = text.as_bytes();
    (0..b.len()).find(|&i| {
        b[i] == b'='
            && !(i > 0 && matches!(b[i - 1], b'<' | b'>' | b'!' | b'='))
            && !(i + 1 < b.len() && b[i + 1] == b'=')
    })
}

fn names(inner: &str) -> Vec<String> {
    inner.split(',').map(|s| s.trim().to_string()).collect()
}

fn parse_law(text: &str, part: &PartDef) -> Result<Law, String> {
    let t = text.trim();
    let scope = LawScope { part };
    let check = |e: &Expr, want: Unit, what: &str| -> Result<(), String> {
        let ty = expr::check(e, Dialect::Law, &scope).map_err(|e| e.message)?;
        if ty.fits(want) {
            Ok(())
        } else {
            Err(format!("{what} gives {}, but it must be in {}", ty.dim().describe(), Dim::of(want).describe()))
        }
    };
    if let Some(rest) = t.strip_prefix("require").filter(|r| r.starts_with(char::is_whitespace)) {
        return parse_require(t, rest.trim(), &scope).map(Law::Require);
    }
    let eq = assignment(t)
        .ok_or("a law is `V(out) = …`, `I(a, b) = …`, `R(a, b) = …`, `C(a, b) = …`, `Rout(out) = …` or `require …`")?;
    let (lhs, rhs) = (t[..eq].trim(), t[eq + 1..].trim());
    let (kind, inner) = lhs
        .strip_suffix(')')
        .and_then(|l| l.split_once('('))
        .ok_or_else(|| format!("\"{lhs}\" is not V(…), I(…), R(…), C(…) or Rout(…)"))?;
    let ns = names(inner);
    for n in &ns {
        if !scope.node(n) {
            return Err(format!("{lhs}: there is no pin {n}"));
        }
    }
    let rhs_e = expr::parse(rhs, Dialect::Law).map_err(|e| e.message)?;
    let one_or_two = |ns: &[String]| -> Result<(String, Option<String>), String> {
        match ns {
            [a] => Ok((a.clone(), None)),
            [a, b] if a != b => Ok((a.clone(), Some(b.clone()))),
            _ => Err(format!("{lhs} names one pin or two different pins")),
        }
    };
    let constant = |e: &Expr| -> Result<(), String> {
        if expr::is_dynamic(e) { Err(format!("{lhs} must be a constant (params and numbers only)")) } else { Ok(()) }
    };
    match kind.trim() {
        "V" => {
            let (pos, neg) = one_or_two(&ns)?;
            let rhs = match &rhs_e {
                Expr::Call(f, args) if f == "tf" => {
                    check(&rhs_e, Unit::Volt, lhs)?;
                    tf_parts(args, None)?
                }
                Expr::Call(f, args) if f == "clamp" && matches!(&args[..], [Expr::Call(g, _), _, _] if g == "tf") => {
                    let Expr::Call(_, tf_args) = &args[0] else { unreachable!() };
                    check(&args[0], Unit::Volt, lhs)?;
                    check(&args[1], Unit::Volt, "the lower limit")?;
                    check(&args[2], Unit::Volt, "the upper limit")?;
                    tf_parts(tf_args, Some((args[1].clone(), args[2].clone())))?
                }
                _ => {
                    check(&rhs_e, Unit::Volt, lhs)?;
                    Rhs::Plain(rhs_e)
                }
            };
            Ok(Law::Volt { pos, neg, rhs })
        }
        "I" => {
            let (from, to) = one_or_two(&ns)?;
            check(&rhs_e, Unit::Ampere, lhs)?;
            Ok(Law::Curr { from, to, value: rhs_e })
        }
        "R" | "C" => {
            let (a, b) = one_or_two(&ns)?;
            let unit = if kind.trim() == "R" { Unit::Ohm } else { Unit::Farad };
            check(&rhs_e, unit, lhs)?;
            constant(&rhs_e)?;
            Ok(if unit == Unit::Ohm { Law::Res { a, b, value: rhs_e } } else { Law::Cap { a, b, value: rhs_e } })
        }
        "Rout" => {
            let [pin] = &ns[..] else { return Err("Rout(pin) names one pin".into()) };
            check(&rhs_e, Unit::Ohm, lhs)?;
            constant(&rhs_e)?;
            Ok(Law::Rout { pin: pin.clone(), value: rhs_e })
        }
        other => Err(format!("\"{other}(…)\" is not a law: use V, I, R, C or Rout")),
    }
}

fn tf_parts(args: &[Expr], clamp: Option<(Expr, Expr)>) -> Result<Rhs, String> {
    let list = |e: &Expr| match e {
        Expr::List(items) => Ok(items.clone()),
        _ => Err("tf(x, [num], [den]): coefficients go in [ ]".to_string()),
    };
    let (num, den) = (list(&args[1])?, list(&args[2])?);
    if den.len() > MAX_ORDER + 1 {
        return Err(format!("a transfer function is at most order {MAX_ORDER}"));
    }
    if num.len() > den.len() {
        return Err("a transfer function's numerator cannot be of higher order than its denominator".into());
    }
    if clamp.is_some() && num.len() != 1 {
        return Err("a clamped tf has a constant numerator (all poles), so one state carries its output".into());
    }
    Ok(Rhs::Tf { x: args[0].clone(), num, den, clamp })
}

fn parse_require(text: &str, rest: &str, scope: &LawScope) -> Result<Require, String> {
    let ty = |e: &Expr| expr::check(e, Dialect::Law, scope).map_err(|e| e.message);
    // `value in [lo, hi]`
    if let Some(k) = rest.rfind(" in ") {
        let (v, range) = (rest[..k].trim(), rest[k + 4..].trim());
        if range.starts_with('[') {
            let value = expr::parse(v, Dialect::Law).map_err(|e| e.message)?;
            let Expr::List(bounds) = expr::parse(range, Dialect::Law).map_err(|e| e.message)? else {
                return Err("require x in [lo, hi]".into());
            };
            let [lo, hi] = &bounds[..] else { return Err("require x in [lo, hi] takes two bounds".into()) };
            let tv = ty(&value)?;
            for b in [lo, hi] {
                if expr::is_dynamic(b) {
                    return Err("a require's bounds are constants".into());
                }
                let tb = ty(b)?;
                if !(tb == Ty::Free || tv == Ty::Free || tb == tv) {
                    return Err(format!(
                        "{b} is in {}, but {value} is in {}",
                        tb.dim().describe(),
                        tv.dim().describe()
                    ));
                }
            }
            return Ok(Require { text: text.to_string(), value, lo: Some(lo.clone()), hi: Some(hi.clone()) });
        }
    }
    let e = expr::parse(rest, Dialect::Law).map_err(|e| e.message)?;
    ty(&e)?;
    let Expr::Bin(op, a, b) = &e else { return Err("require compares a value with a constant: I(out) < 20m".into()) };
    let (value, bound, below) = match (expr::is_dynamic(a), expr::is_dynamic(b)) {
        (true, false) => (a, b, matches!(op, BinOp::Lt | BinOp::Le)),
        (false, true) => (b, a, matches!(op, BinOp::Gt | BinOp::Ge)),
        _ => return Err("require compares one reading (V, I) with a constant".into()),
    };
    if !matches!(op, BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge) {
        return Err("require uses <, <=, > or >=".into());
    }
    let (lo, hi) = if below { (None, Some((**bound).clone())) } else { (Some((**bound).clone()), None) };
    Ok(Require { text: text.to_string(), value: (**value).clone(), lo, hi })
}

/// Parse and check every law against the component's pins and params; every problem is listed.
pub fn parse(b: &Behavioral, part: &PartDef) -> Result<Laws, Vec<String>> {
    let mut errs = Vec::new();
    if b.laws.is_empty() {
        errs.push("a behavioral component needs at least one law".into());
    }
    if b.laws.len() > MAX_LAWS {
        errs.push(format!("at most {MAX_LAWS} laws"));
    }
    for p in &part.pins {
        if p.name.to_ascii_lowercase().starts_with(RESERVED) {
            errs.push(format!("pin {}: names starting {} are reserved", p.name, RESERVED.to_ascii_uppercase()));
        }
    }
    let mut laws = Vec::new();
    for (i, text) in b.laws.iter().enumerate() {
        match parse_law(text, part) {
            Ok(l) => laws.push(l),
            Err(e) => errs.push(format!("law {}: {e}", i + 1)),
        }
    }
    let laws = Laws { laws };
    // Each pin is driven, loaded or read by some law, and each output has one source.
    let mut used = BTreeSet::new();
    let mut driven: BTreeMap<String, usize> = BTreeMap::new();
    let mut routs = BTreeSet::new();
    for (i, l) in laws.laws.iter().enumerate() {
        for n in law_pins(l) {
            used.insert(n);
        }
        match l {
            Law::Volt { pos, .. } => {
                if let Some(j) = driven.insert(pos.clone(), i + 1) {
                    errs.push(format!("law {}: V({pos}) is already set by law {j}", i + 1));
                }
            }
            Law::Rout { pin, .. } if !routs.insert(pin.clone()) => {
                errs.push(format!("law {}: Rout({pin}) is set twice", i + 1));
            }
            _ => {}
        }
    }
    for pin in &routs {
        if !driven.contains_key(pin) {
            errs.push(format!("Rout({pin}) needs a law V({pin}) = …"));
        }
    }
    for p in &part.pins {
        if p.kind != crate::registry::PinType::Nc && !used.contains(&p.name) {
            errs.push(format!("pin {} is in no law", p.name));
        }
    }
    if errs.is_empty() { Ok(laws) } else { Err(errs) }
}

/// Pins a law names or reads.
fn law_pins(l: &Law) -> Vec<String> {
    let mut out = Vec::new();
    let mut read = |e: &Expr| {
        expr::walk(e, &mut |x| match x {
            Expr::Volt(a, b) => {
                out.push(a.clone());
                out.extend(b.clone());
            }
            Expr::Amp(a) => out.push(a.clone()),
            _ => {}
        })
    };
    let mut named = Vec::new();
    match l {
        Law::Volt { pos, neg, rhs } => {
            named.push(pos.clone());
            named.extend(neg.clone());
            match rhs {
                Rhs::Plain(e) => read(e),
                Rhs::Tf { x, clamp, .. } => {
                    read(x);
                    if let Some((lo, hi)) = clamp {
                        read(lo);
                        read(hi);
                    }
                }
            }
        }
        Law::Curr { from, to, value } => {
            named.push(from.clone());
            named.extend(to.clone());
            read(value);
        }
        Law::Res { a, b, .. } | Law::Cap { a, b, .. } => {
            named.push(a.clone());
            named.extend(b.clone());
        }
        Law::Rout { pin, .. } => named.push(pin.clone()),
        Law::Require(r) => read(&r.value),
    }
    named.extend(out);
    named.retain(|n| n != crate::ir::GND);
    named
}

/// Pairs of pins (or a pin and ground, `None`) joined by a DC path inside: voltage laws, resistors
/// and output resistances conduct; currents and capacitors do not.
pub fn conducting(laws: &Laws) -> Vec<(String, Option<String>)> {
    let ground = |n: &Option<String>| n.clone().filter(|x| x != crate::ir::GND);
    let mut out = Vec::new();
    for l in &laws.laws {
        match l {
            Law::Volt { pos, neg, .. } => out.push((pos.clone(), ground(neg))),
            Law::Res { a, b, .. } => out.push((a.clone(), ground(b))),
            _ => {}
        }
    }
    out.retain(|(a, _)| a != crate::ir::GND);
    out
}

/// A require as compiled into one netlist: the internal node holding its value and its bounds.
#[derive(Clone, Debug, PartialEq)]
pub struct Envelope {
    pub law: usize,
    pub text: String,
    pub node: String,
    pub unit: Unit,
    pub lo: Option<f64>,
    pub hi: Option<f64>,
}

/// The body of the `.subckt` for these laws at these param values: element lines, the requires'
/// nodes, and the count of primitive devices.
pub fn emit(
    laws: &Laws,
    part: &PartDef,
    params: &BTreeMap<String, f64>,
) -> Result<(Vec<String>, Vec<Envelope>), String> {
    let num = |e: &Expr, what: &str| -> Result<f64, String> {
        let v = expr::eval(e, params).map_err(|e| format!("{what}: {e}"))?.value;
        if v.is_finite() { Ok(v) } else { Err(format!("{what} is not finite at these params")) }
    };
    // Pins whose current a law reads get a zero-volt sense source; elements attach behind it.
    let mut sensed = BTreeSet::new();
    for l in &laws.laws {
        let mut look = |e: &Expr| {
            expr::walk(e, &mut |x| {
                if let Expr::Amp(p) = x {
                    sensed.insert(p.clone());
                }
            })
        };
        match l {
            Law::Volt { rhs: Rhs::Plain(e), .. } | Law::Curr { value: e, .. } => look(e),
            Law::Volt { rhs: Rhs::Tf { x, clamp, .. }, .. } => {
                look(x);
                if let Some((a, b)) = clamp {
                    look(a);
                    look(b);
                }
            }
            Law::Require(r) => look(&r.value),
            _ => {}
        }
    }
    let node = |pin: &str| -> String {
        if pin == crate::ir::GND {
            "0".into()
        } else if sensed.contains(pin) {
            format!("{RESERVED}i_{}", pin.to_ascii_lowercase())
        } else {
            pin.to_ascii_lowercase()
        }
    };
    let neg = |n: &Option<String>| n.as_deref().map_or("0".to_string(), node);
    let names = LawNames { params, extra: BTreeMap::new() };

    let mut lines = Vec::new();
    for p in &sensed {
        let l = p.to_ascii_lowercase();
        lines.push(format!("V{RESERVED}s_{l} {l} {} 0", node(p)));
    }
    let routs: BTreeMap<&str, &Expr> = laws
        .laws
        .iter()
        .filter_map(|l| match l {
            Law::Rout { pin, value } => Some((pin.as_str(), value)),
            _ => None,
        })
        .collect();
    let mut envelopes = Vec::new();
    for (i, l) in laws.laws.iter().enumerate() {
        let k = i + 1;
        let positive =
            |v: f64, what: &str| if v > 0.0 { Ok(v) } else { Err(format!("law {k}: {what} must be positive")) };
        match l {
            Law::Volt { pos, neg: n, rhs } => {
                let out = match routs.get(pos.as_str()) {
                    Some(r) => {
                        let ro = positive(num(r, &format!("Rout({pos})"))?, "Rout")?;
                        let o = format!("{RESERVED}o{k}");
                        lines.push(format!("R{RESERVED}o{k} {o} {} {}", node(pos), spice_number(ro)));
                        o
                    }
                    None => node(pos),
                };
                let value = match rhs {
                    Rhs::Plain(e) => expr::emit(e, &names).map_err(|e| format!("law {k}: {e}"))?,
                    Rhs::Tf { x, num: b, den: a, clamp } => {
                        let (states, y) = state_space(k, x, b, a, clamp.as_ref(), params, &names)?;
                        lines.extend(states);
                        y
                    }
                };
                lines.push(format!("B{RESERVED}l{k} {out} {} V={value}", neg(n)));
            }
            Law::Curr { from, to, value } => {
                if expr::is_dynamic(value) {
                    let v = expr::emit(value, &names).map_err(|e| format!("law {k}: {e}"))?;
                    lines.push(format!("B{RESERVED}l{k} {} {} I={v}", node(from), neg(to)));
                } else {
                    let v = num(value, &format!("law {k}"))?;
                    lines.push(format!("I{RESERVED}l{k} {} {} DC {}", node(from), neg(to), spice_number(v)));
                }
            }
            Law::Res { a, b, value } => {
                let v = positive(num(value, &format!("law {k}"))?, "a resistance")?;
                lines.push(format!("R{RESERVED}l{k} {} {} {}", node(a), neg(b), spice_number(v)));
            }
            Law::Cap { a, b, value } => {
                let v = positive(num(value, &format!("law {k}"))?, "a capacitance")?;
                lines.push(format!("C{RESERVED}l{k} {} {} {}", node(a), neg(b), spice_number(v)));
            }
            Law::Rout { .. } => {}
            Law::Require(r) => {
                let v = expr::emit(&r.value, &names).map_err(|e| format!("law {k}: {e}"))?;
                let n = format!("{RESERVED}q{k}");
                lines.push(format!("B{RESERVED}q{k} {n} 0 V={v}"));
                let scope = LawScope { part };
                let unit = expr::check(&r.value, Dialect::Law, &scope)
                    .ok()
                    .and_then(|t| t.dim().unit())
                    .unwrap_or(Unit::Unitless);
                let bound = |b: &Option<Expr>| b.as_ref().map(|e| num(e, &format!("law {k}"))).transpose();
                envelopes.push(Envelope {
                    law: k,
                    text: r.text.clone(),
                    node: n,
                    unit,
                    lo: bound(&r.lo)?,
                    hi: bound(&r.hi)?,
                });
            }
        }
    }
    Ok((lines, envelopes))
}

/// Names inside a law: params are their numbers, pins their nodes, `I(pin)` the sense source,
/// and `extra` the state nodes of a transfer function.
struct LawNames<'a> {
    params: &'a BTreeMap<String, f64>,
    extra: BTreeMap<String, String>,
}

impl Names for LawNames<'_> {
    fn var(&self, name: &str) -> Option<String> {
        self.params.get(name).map(|v| if *v < 0.0 { format!("({})", spice_number(*v)) } else { spice_number(*v) })
    }
    fn node(&self, name: &str) -> Option<String> {
        if let Some(n) = self.extra.get(name) {
            return Some(n.clone());
        }
        Some(if name == crate::ir::GND { "0".into() } else { name.to_ascii_lowercase() })
    }
    fn current(&self, name: &str) -> Option<String> {
        Some(format!("v{RESERVED}s_{}", name.to_ascii_lowercase()))
    }
}

/// Four significant digits for a value only a transcendental function can produce, so the
/// netlist text is identical on every runtime.
fn settle(x: f64) -> f64 {
    if x == 0.0 || !x.is_finite() { x } else { format!("{x:.3e}").parse().expect("re-parses") }
}

/// ω0 = (a0/an)^(1/n) without libm where possible: exact for orders 1, 2 and 4, rounded to four
/// digits otherwise (any positive scale realises the same function).
fn natural(ratio: f64, n: usize) -> f64 {
    match n {
        1 => ratio,
        2 => ratio.sqrt(),
        4 => ratio.sqrt().sqrt(),
        _ => settle(ratio.powf(1.0 / n as f64)),
    }
}

/// Whole powers as repeated products, as the expression language computes them.
fn powi(x: f64, n: usize) -> f64 {
    let mut p = 1.0;
    for _ in 0..n {
        p *= x;
    }
    p
}

/// State elements of `tf(x, num, den)` (coefficients highest power first) and the output
/// expression. States z1…zn in time τ = ω0·t: z_i' = z_{i+1}, z_n' = u − Σ ã_k z_{k+1};
/// y = Σ (b̃_k − b̃_n ã_k) z_{k+1} + b̃_n u. Each state is a B current into a capacitor of 1/ω0.
fn state_space(
    k: usize,
    x: &Expr,
    num: &[Expr],
    den: &[Expr],
    clamp: Option<&(Expr, Expr)>,
    params: &BTreeMap<String, f64>,
    names: &LawNames,
) -> Result<(Vec<String>, String), String> {
    let coeff = |e: &Expr| -> Result<f64, String> {
        let v = expr::eval(e, params).map_err(|e| format!("law {k}: {e}"))?.value;
        if v.is_finite() { Ok(v) } else { Err(format!("law {k}: a tf coefficient is not finite")) }
    };
    // Ascending powers.
    let a: Vec<f64> = den.iter().rev().map(coeff).collect::<Result<_, _>>()?;
    let mut b: Vec<f64> = num.iter().rev().map(coeff).collect::<Result<_, _>>()?;
    let n = a.len() - 1;
    b.resize(n + 1, 0.0);
    if a[n] == 0.0 || a[0] == 0.0 {
        return Err(format!(
            "law {k}: tf needs non-zero first and last denominator coefficients (a pure integrator has no operating point)"
        ));
    }
    let w0 = natural((a[0] / a[n]).abs(), n);
    let scale = a[n] * powi(w0, n);
    let at: Vec<f64> = (0..=n).map(|i| a[i] * powi(w0, i) / scale).collect();
    let bt: Vec<f64> = (0..=n).map(|i| b[i] * powi(w0, i) / scale).collect();
    let u = expr::emit(x, names).map_err(|e| format!("law {k}: {e}"))?;
    let z = |i: usize| format!("{RESERVED}s{k}_{i}");
    let v = |i: usize| format!("v({})", z(i));
    let c = spice_number(1.0 / w0);
    let term = |coef: f64, s: String| -> Option<String> {
        if coef == 0.0 {
            None
        } else if coef < 0.0 {
            Some(format!("(({})*{s})", spice_number(coef)))
        } else {
            Some(format!("({}*{s})", spice_number(coef)))
        }
    };
    let mut lines = Vec::new();
    for i in 1..=n {
        let drive = if i < n {
            v(i + 1)
        } else {
            let fb: Vec<String> = (0..n).filter_map(|j| term(at[j], v(j + 1))).collect();
            if fb.is_empty() { format!("({u})") } else { format!("({u}-({}))", fb.join("+")) }
        };
        let current = match clamp {
            Some((lo, hi)) if i == 1 => {
                // y = c·z1: hold z1 inside lo/c … hi/c, swapping the bounds when c < 0.
                let gain = bt[0];
                if gain == 0.0 {
                    return Err(format!("law {k}: a clamped tf with a zero numerator"));
                }
                let lo = expr::emit(lo, names).map_err(|e| format!("law {k}: {e}"))?;
                let hi = expr::emit(hi, names).map_err(|e| format!("law {k}: {e}"))?;
                let g = spice_number(gain);
                let (zlo, zhi) = if gain > 0.0 {
                    (format!("({lo}/{g})"), format!("({hi}/{g})"))
                } else {
                    (format!("({hi}/{g})"), format!("({lo}/{g})"))
                };
                let kk = spice_number(BOUND_GAIN);
                format!("min(max({drive},{kk}*({zlo}-{})),{kk}*({zhi}-{}))", v(1), v(1))
            }
            _ => drive,
        };
        lines.push(format!("C{RESERVED}s{k}_{i} {} 0 {c}", z(i)));
        lines.push(format!("B{RESERVED}s{k}_{i} 0 {} I={current}", z(i)));
    }
    let mut out: Vec<String> = (0..n).filter_map(|j| term(bt[j] - bt[n] * at[j], v(j + 1))).collect();
    if let Some(t) = term(bt[n], format!("({u})")) {
        out.push(t);
    }
    let y = if out.is_empty() { "0".to_string() } else { format!("({})", out.join("+")) };
    let y = match clamp {
        Some((lo, hi)) => {
            let lo = expr::emit(lo, names).map_err(|e| format!("law {k}: {e}"))?;
            let hi = expr::emit(hi, names).map_err(|e| format!("law {k}: {e}"))?;
            format!("min(max({y},{lo}),{hi})")
        }
        None => y,
    };
    Ok((lines, y))
}

/// Primitive devices in an emitted body.
pub fn leaf_devices(lines: &[String]) -> u64 {
    lines.len() as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    fn part(pins: &[(&str, &str)], params: &[(&str, &str, &str)]) -> PartDef {
        let pins: Vec<String> =
            pins.iter().enumerate().map(|(i, (n, t))| format!("{{name: {n}, num: {}, type: {t}}}", i + 1)).collect();
        let params: Vec<String> =
            params.iter().map(|(n, u, d)| format!("{n}: {{unit: {u}, default: \"{d}\"}}")).collect();
        let yaml = format!(
            "{{id: \"lib:t\", category: X, title: T, pins: [{}], params: {{{}}}, symbol: auto}}",
            pins.join(", "),
            params.join(", ")
        );
        serde_norway::from_str(&yaml).unwrap()
    }

    fn opamp() -> PartDef {
        part(
            &[("inp", "input"), ("inm", "input"), ("out", "output"), ("vcc", "power_pos"), ("vee", "power_neg")],
            &[("aol", "unitless", "100k"), ("fp", "hertz", "10")],
        )
    }

    #[test]
    fn laws_parse_and_check_units() {
        let p = opamp();
        let ok = Behavioral {
            laws: vec![
                "V(out) = clamp(tf(aol*(V(inp) - V(inm)), [1], [1/(2*pi*fp), 1]), V(vee) + 0.02, V(vcc) - 1.5)".into(),
                "R(inp, inm) = 1Meg".into(),
                "Rout(out) = 50".into(),
                "I(vcc, vee) = 0.7m".into(),
                "require V(vcc) - V(vee) in [3, 32]".into(),
                "require I(out) < 20m".into(),
            ],
        };
        let laws = parse(&ok, &p).unwrap();
        assert_eq!(laws.laws.len(), 6);
        assert!(matches!(&laws.laws[5], Law::Require(r) if r.lo.is_none() && r.hi.is_some()));
        assert_eq!(conducting(&laws), [("out".to_string(), None), ("inp".to_string(), Some("inm".to_string()))]);

        let bad =
            Behavioral { laws: vec!["V(out) = V(inp) + I(out)".into(), "R(inp) = V(inm)".into(), "Q(out) = 1".into()] };
        let errs = parse(&bad, &p).unwrap_err();
        assert_eq!(errs[0], "law 1: cannot add V(inp) (volts) and I(out) (amps)");
        assert!(errs[1].starts_with("law 2: R(inp) gives volts"), "{errs:?}");
        assert!(errs[2].contains("is not a law"), "{errs:?}");
        assert!(errs.iter().any(|e| e == "pin vcc is in no law"), "{errs:?}");
    }

    #[test]
    fn bounded_state_space_netlist() {
        let p = opamp();
        let b = Behavioral {
            laws: vec![
                "V(out) = clamp(tf(aol*(V(inp) - V(inm)), [1], [1/(2*pi*fp), 1]), V(vee) + 0.02, V(vcc) - 1.5)".into(),
                "R(inp, inm) = 1Meg".into(),
                "Rout(out) = 50".into(),
            ],
        };
        let laws = parse(&b, &p).unwrap();
        let params = BTreeMap::from([("aol".to_string(), 1e5), ("fp".to_string(), 10.0)]);
        let (lines, env) = emit(&laws, &p, &params).unwrap();
        assert!(env.is_empty());
        let w0 = 1.0 / (1.0 / (2.0 * std::f64::consts::PI * 10.0));
        let text = lines.join("\n");
        assert!(text.contains(&format!("Cbx_s1_1 bx_s1_1 0 {}", spice_number(1.0 / w0))), "{text}");
        assert!(text.contains("min(max(((1e5*(v(inp)-v(inm)))-((1e0*v(bx_s1_1)))),1e6*"), "{text}");
        assert!(text.contains("Rbx_o1 bx_o1 out 5e1"), "{text}");
        assert!(text.contains("Bbx_l1 bx_o1 0 V=min(max(((1e0*v(bx_s1_1))),(v(vee)+2e-2)),(v(vcc)-1.5e0))"), "{text}");
        assert!(text.contains("Rbx_l2 inp inm 1e6"), "{text}");
    }
}
