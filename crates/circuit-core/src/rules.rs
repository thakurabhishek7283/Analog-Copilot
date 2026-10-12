//! Value rules (LLD §17 D5): part values from targets, written as data instead of a Rust solver.
//!
//! An author writes ordered steps and forward estimates; the core searches the E-series:
//!
//! ```yaml
//! rules:
//!   solve:
//!     tau: "1/(2*pi*fc_hz)"                 # an intermediate value (lowercase name)
//!     C1: "search(E12, 100p, 10u)"          # every series value in turn
//!     R1: "nearest(E24, tau/C1, 1k, 100k)"  # the nearest series value; outside 1k..100k, no candidate
//!   estimates: {tau: "R1*C1"}               # values -> spec, scored against the step or target `tau`
//!   prefer: {R1: 10k}                       # a tie-breaker towards 10k
//! ```
//!
//! Steps run in order, so a `search` range may depend on earlier steps (Sallen-Key's C1 range
//! follows C2). Each complete set of values is a candidate scored by the summed squared relative
//! error of every estimate, plus a tiny pull towards each preferred value; the first candidate
//! that no later one beats by more than 1e-9 wins, as in the Rust solvers ([`crate::template::solvers`]),
//! which the rules reproduce operation for operation. More than [`MAX_CANDIDATES`] candidates is
//! an authoring error. Results of transcendental functions are rounded to 4 significant digits
//! before they choose or become a part value, so every runtime picks the same part.

use std::collections::BTreeMap;

use indexmap::IndexMap;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::expr::{self, Dialect, Dim, Expr, Func, Scope, Ty};
use crate::registry::PartDef;
use crate::units::{ESeries, Unit};

pub const MAX_CANDIDATES: usize = 100_000;

#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, Default, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ValueRules {
    /// User functions, `"name(a, b)" -> body`; a function may call those before it.
    #[serde(default, skip_serializing_if = "IndexMap::is_empty")]
    pub funcs: IndexMap<String, String>,
    /// Steps in order. A part's local refdes (`R1`, or `R1.resistance` for a part with several
    /// params) sets that part value; any other name (lowercase) is an intermediate value.
    pub solve: IndexMap<String, String>,
    /// Forward estimates, each scored against the target or step of the same name.
    #[serde(default, skip_serializing_if = "IndexMap::is_empty")]
    pub estimates: IndexMap<String, String>,
    /// Steps to keep near a nominal value when the estimates tie.
    #[serde(default, skip_serializing_if = "IndexMap::is_empty")]
    pub prefer: IndexMap<String, String>,
    /// Spec targets derived from the inputs and the chosen values (a follower's gain of 1).
    #[serde(default, skip_serializing_if = "IndexMap::is_empty")]
    pub spec: IndexMap<String, String>,
}

#[derive(Clone, Debug, PartialEq)]
enum Kind {
    Value(Expr),
    Search { series: ESeries, lo: Expr, hi: Expr, keep: Option<(Expr, Expr)> },
    Nearest { series: ESeries, x: Expr, keep: Option<(Expr, Expr)> },
}

#[derive(Clone, Debug, PartialEq)]
struct Step {
    name: String,
    slot: usize,
    /// The part value it sets: local refdes and param.
    part: Option<(String, String)>,
    kind: Kind,
}

/// Rules parsed and dimension-checked against their inputs and parts, ready to solve.
#[derive(Clone, Debug, PartialEq)]
pub struct Rules {
    /// Inputs by name, each with its slot.
    inputs: Vec<(String, usize)>,
    slots: usize,
    steps: Vec<Step>,
    /// (name, slot of the target or step it is scored against, estimate).
    estimates: Vec<(String, usize, Expr)>,
    prefer: Vec<(String, usize, Expr)>,
    spec: Vec<(String, Expr)>,
}

/// Chosen values per local refdes and param, and the derived spec targets, in SI units.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Solved {
    pub values: BTreeMap<String, BTreeMap<String, f64>>,
    pub spec: BTreeMap<String, f64>,
    /// How many candidates were scored.
    pub candidates: usize,
}

struct Types<'a> {
    known: &'a BTreeMap<String, Ty>,
}

impl Scope for Types<'_> {
    fn var(&self, name: &str) -> Option<Ty> {
        self.known.get(name).copied()
    }
    fn series(&self) -> bool {
        true
    }
}

fn is_local_refdes(s: &str) -> bool {
    let b = s.as_bytes();
    !b.is_empty() && b[0].is_ascii_uppercase()
}

impl ValueRules {
    /// Parse and check every step against the inputs (targets and rails, with their units) and
    /// the parts whose values the rules may set (local refdes -> part); with `require_all`, every
    /// param of every one of those parts must be set. Every problem is listed.
    pub fn compile(
        &self,
        inputs: &BTreeMap<String, Unit>,
        parts: &BTreeMap<String, &PartDef>,
        require_all: bool,
    ) -> Result<Rules, Vec<String>> {
        let mut errs = Vec::new();
        let mut funcs: BTreeMap<String, Func> = BTreeMap::new();
        for (head, body) in &self.funcs {
            match expr::parse_head(head).and_then(|(n, ps)| {
                let b = expr::inline(&expr::parse(body, Dialect::Law)?, &funcs)?;
                Ok((n, Func { params: ps, body: b }))
            }) {
                Ok((n, f)) => {
                    funcs.insert(n, f);
                }
                Err(e) => errs.push(format!("func {head}: {e}")),
            }
        }
        let parse = |what: &str, text: &str, errs: &mut Vec<String>| -> Option<Expr> {
            match expr::parse(text, Dialect::Law).and_then(|e| expr::inline(&e, &funcs)) {
                Ok(e) => Some(e),
                Err(e) => {
                    errs.push(format!("{what}: {e}"));
                    None
                }
            }
        };

        let mut known: BTreeMap<String, Ty> = inputs.iter().map(|(k, u)| (k.clone(), Ty::Dim(Dim::of(*u)))).collect();
        let mut steps = Vec::new();
        let mut set: BTreeMap<(String, String), String> = BTreeMap::new();
        for (name, text) in &self.solve {
            if known.contains_key(name) {
                errs.push(format!("step {name}: the name is already an input or an earlier step"));
                continue;
            }
            // Which part value, if any, the step sets.
            let part = if is_local_refdes(name) {
                let (local, param) = match name.split_once('.') {
                    Some((l, p)) => (l, Some(p)),
                    None => (name.as_str(), None),
                };
                let Some(pd) = parts.get(local) else {
                    errs.push(format!("step {name}: there is no part {local}"));
                    continue;
                };
                let param = match param {
                    Some(p) if pd.params.contains_key(p) => p.to_string(),
                    Some(p) => {
                        errs.push(format!("step {name}: {local} has no param {p}"));
                        continue;
                    }
                    None if pd.params.len() == 1 => pd.params.keys().next().expect("one").clone(),
                    None => {
                        let names: Vec<&str> = pd.params.keys().map(String::as_str).collect();
                        errs.push(format!(
                            "step {name}: {local} has params {}; name one ({local}.x)",
                            names.join(", ")
                        ));
                        continue;
                    }
                };
                if let Some(other) = set.insert((local.to_string(), param.clone()), name.clone()) {
                    errs.push(format!("step {name}: {other} already sets {local}.{param}"));
                }
                Some((local.to_string(), param))
            } else {
                None
            };
            let Some(e) = parse(&format!("step {name}"), text, &mut errs) else { continue };
            let unit = part.as_ref().map(|(l, p)| parts[l.as_str()].params[p].unit);
            let types = Types { known: &known };
            let checked = |e: &Expr| expr::check(e, Dialect::Law, &types);
            let kind = match &e {
                Expr::Call(f, args) if f == "search" || f == "nearest" => {
                    let series = match &args[0] {
                        Expr::Var(s) => expr::series_of(s),
                        _ => None,
                    };
                    let Some(series) = series else {
                        errs.push(format!("step {name}: {f}'s first argument is E6, E12 or E24"));
                        continue;
                    };
                    let rest = &args[1..];
                    let keep = |i: usize| rest.get(i).cloned().zip(rest.get(i + 1).cloned());
                    let ok = match f.as_str() {
                        "search" => matches!(rest.len(), 2 | 4),
                        _ => matches!(rest.len(), 1 | 3),
                    };
                    if !ok {
                        errs.push(format!(
                            "step {name}: write {}",
                            if f == "search" {
                                "search(E12, lo, hi) or search(E12, lo, hi, min, max)"
                            } else {
                                "nearest(E24, x) or nearest(E24, x, min, max)"
                            }
                        ));
                        continue;
                    }
                    if f == "search" {
                        Kind::Search { series, lo: rest[0].clone(), hi: rest[1].clone(), keep: keep(2) }
                    } else {
                        Kind::Nearest { series, x: rest[0].clone(), keep: keep(1) }
                    }
                }
                _ => Kind::Value(e.clone()),
            };
            match checked(&e) {
                Ok(t) => {
                    if let Some(u) = unit
                        && !t.fits(u)
                    {
                        errs.push(format!(
                            "step {name}: gives {}, but {name} is in {}",
                            t.dim().describe(),
                            Dim::of(u).describe()
                        ));
                    }
                    // A part value has its param's unit, whatever the step got wrong.
                    known.insert(name.clone(), unit.map_or(t, |u| Ty::Dim(Dim::of(u))));
                }
                Err(e) => {
                    errs.push(format!("step {name}: {e}"));
                    continue;
                }
            }
            steps.push(Step { name: name.clone(), slot: 0, part, kind });
        }

        let types = Types { known: &known };
        let scored = |what: &str, map: &IndexMap<String, String>, errs: &mut Vec<String>| {
            let mut out = Vec::new();
            for (name, text) in map {
                let Some(e) = parse(&format!("{what} {name}"), text, errs) else { continue };
                let Some(want) = known.get(name) else {
                    errs.push(format!("{what} {name}: no target or step is called {name}"));
                    continue;
                };
                match expr::check(&e, Dialect::Law, &types) {
                    Ok(t) if t == Ty::Free || *want == Ty::Free || t == *want => out.push((name.clone(), e)),
                    Ok(t) => errs.push(format!(
                        "{what} {name}: gives {}, but {name} is in {}",
                        t.dim().describe(),
                        want.dim().describe()
                    )),
                    Err(e) => errs.push(format!("{what} {name}: {e}")),
                }
            }
            out
        };
        let estimates = scored("estimate", &self.estimates, &mut errs);
        let prefer = scored("prefer", &self.prefer, &mut errs);
        let mut spec = Vec::new();
        for (name, text) in &self.spec {
            let Some(e) = parse(&format!("spec {name}"), text, &mut errs) else { continue };
            match expr::check(&e, Dialect::Law, &types) {
                Ok(_) => spec.push((name.clone(), e)),
                Err(e) => errs.push(format!("spec {name}: {e}")),
            }
        }
        // Every param of every part gets a value.
        for (local, pd) in parts.iter().filter(|_| require_all) {
            for p in pd.params.keys() {
                if !set.contains_key(&(local.clone(), p.clone())) {
                    errs.push(format!("no step sets {local}.{p}"));
                }
            }
        }
        if !errs.is_empty() {
            return Err(errs);
        }
        // Slots: inputs first, then steps in order.
        let mut index: BTreeMap<String, usize> = BTreeMap::new();
        for k in inputs.keys() {
            let n = index.len();
            index.insert(k.clone(), n);
        }
        for st in &steps {
            let n = index.len();
            index.insert(st.name.clone(), n);
        }
        let b = |e: &Expr| expr::bind(e, &|n| index.get(n).copied());
        let bk = |k: &Kind| match k {
            Kind::Value(e) => Kind::Value(b(e)),
            Kind::Search { series, lo, hi, keep } => {
                Kind::Search { series: *series, lo: b(lo), hi: b(hi), keep: keep.as_ref().map(|(x, y)| (b(x), b(y))) }
            }
            Kind::Nearest { series, x, keep } => {
                Kind::Nearest { series: *series, x: b(x), keep: keep.as_ref().map(|(x, y)| (b(x), b(y))) }
            }
        };
        let steps = steps
            .iter()
            .map(|st| Step { name: st.name.clone(), slot: index[&st.name], part: st.part.clone(), kind: bk(&st.kind) })
            .collect();
        let scored = |v: Vec<(String, Expr)>| v.into_iter().map(|(n, e)| (n.clone(), index[&n], b(&e))).collect();
        Ok(Rules {
            inputs: inputs.keys().map(|k| (k.clone(), index[k])).collect(),
            slots: index.len(),
            steps,
            estimates: scored(estimates),
            prefer: scored(prefer),
            spec: spec.into_iter().map(|(n, e)| (n, b(&e))).collect(),
        })
    }
}

/// Values in slots; unset ones are NaN.
struct Slots<'a>(&'a [f64]);

impl expr::Values for Slots<'_> {
    fn var(&self, _: &str) -> Option<f64> {
        None
    }
    fn slot(&self, k: usize) -> Option<f64> {
        self.0.get(k).copied()
    }
}

/// `(actual / wanted − 1)²`.
fn e2(actual: f64, wanted: f64) -> f64 {
    let e = actual / wanted - 1.0;
    e * e
}

/// Zero at `nominal`, growing either way; tiny, so it only breaks ties.
fn near(x: f64, nominal: f64) -> f64 {
    1e-6 * (x / nominal + nominal / x - 2.0)
}

/// Four significant digits for a value a transcendental function produced.
fn settle(r: expr::Eval) -> f64 {
    if r.inexact && r.value.is_finite() && r.value != 0.0 {
        format!("{:.3e}", r.value).parse().expect("{:.3e} re-parses")
    } else {
        r.value
    }
}

struct Search<'a> {
    rules: &'a Rules,
    env: Vec<f64>,
    prefer: Vec<f64>,
    best: Option<(f64, Vec<f64>)>,
    candidates: usize,
}

impl Rules {
    /// Search for the best candidate given input values (targets and rails, SI).
    pub fn solve(&self, inputs: &BTreeMap<String, f64>) -> Result<Solved, String> {
        let mut env = vec![f64::NAN; self.slots];
        for (name, k) in &self.inputs {
            env[*k] = *inputs.get(name).ok_or_else(|| format!("no value for {name}"))?;
        }
        let mut prefer = Vec::new();
        for (name, _, e) in &self.prefer {
            prefer.push(settle(expr::eval(e, &Slots(&env)).map_err(|e| format!("prefer {name}: {e}"))?));
        }
        let mut s = Search { rules: self, env, prefer, best: None, candidates: 0 };
        s.step(0)?;
        let Some((_, env)) = s.best else {
            return Err("no combination of series values meets the rules".into());
        };
        let mut out = Solved { candidates: s.candidates, ..Solved::default() };
        for st in &self.steps {
            if let Some((local, param)) = &st.part {
                out.values.entry(local.clone()).or_default().insert(param.clone(), env[st.slot]);
            }
        }
        for (name, e) in &self.spec {
            let r = expr::eval(e, &Slots(&env)).map_err(|e| format!("spec {name}: {e}"))?;
            out.spec.insert(name.clone(), r.value);
        }
        Ok(out)
    }
}

impl Search<'_> {
    fn eval(&self, what: &str, e: &Expr) -> Result<f64, String> {
        expr::eval(e, &Slots(&self.env)).map(settle).map_err(|e| format!("{what}: {e}"))
    }

    fn keep(&self, name: &str, keep: &Option<(Expr, Expr)>, v: f64) -> Result<bool, String> {
        Ok(match keep {
            Some((lo, hi)) => v >= self.eval(name, lo)? && v <= self.eval(name, hi)?,
            None => true,
        })
    }

    fn step(&mut self, i: usize) -> Result<(), String> {
        let Some(st) = self.rules.steps.get(i) else { return self.score() };
        match &st.kind {
            Kind::Value(e) => {
                let v = self.eval(&st.name, e)?;
                self.env[st.slot] = v;
                self.step(i + 1)
            }
            Kind::Nearest { series, x, keep } => {
                let x = self.eval(&st.name, x)?;
                if !(x.is_finite() && x > 0.0) {
                    return Ok(());
                }
                let v = series.nearest(x);
                if !self.keep(&st.name, keep, v)? {
                    return Ok(());
                }
                self.env[st.slot] = v;
                self.step(i + 1)
            }
            Kind::Search { series, lo, hi, keep } => {
                let (lo, hi) = (self.eval(&st.name, lo)?, self.eval(&st.name, hi)?);
                if !(lo.is_finite() && hi.is_finite() && lo > 0.0 && hi >= lo) {
                    return Ok(());
                }
                for v in series.values(lo, hi) {
                    if !self.keep(&st.name, keep, v)? {
                        continue;
                    }
                    self.env[st.slot] = v;
                    self.step(i + 1)?;
                }
                Ok(())
            }
        }
    }

    fn score(&mut self) -> Result<(), String> {
        self.candidates += 1;
        if self.candidates > MAX_CANDIDATES {
            return Err(format!("the rules try more than {MAX_CANDIDATES} combinations; narrow a search range"));
        }
        let mut score = 0.0;
        for (name, k, e) in &self.rules.estimates {
            let est = expr::eval(e, &Slots(&self.env)).map(settle).map_err(|e| format!("estimate {name}: {e}"))?;
            score += e2(est, self.env[*k]);
        }
        for ((_, k, _), nominal) in self.rules.prefer.iter().zip(&self.prefer) {
            score += near(self.env[*k], *nominal);
        }
        if !score.is_finite() {
            return Ok(());
        }
        if self.best.as_ref().is_none_or(|(s, _)| score < s - 1e-9) {
            self.best = Some((score, self.env.clone()));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rules(yaml: &str) -> ValueRules {
        serde_norway::from_str(yaml).unwrap()
    }

    fn part(yaml: &str) -> PartDef {
        serde_norway::from_str(yaml).unwrap()
    }

    fn parts() -> (PartDef, PartDef) {
        (
            part(include_str!("../../../registry/parts/resistor_th.yaml")),
            part(include_str!("../../../registry/parts/cap_film.yaml")),
        )
    }

    const RC: &str = "solve: {tau: '1/(2*pi*fc_hz)', C1: 'search(E12, 100p, 10u)', R1: 'nearest(E24, tau/C1, 1k, 100k)'}\nestimates: {tau: 'R1*C1'}\nprefer: {R1: 10k}";

    #[test]
    fn rc_solves_like_the_rust_solver() {
        let (r, c) = parts();
        let p = BTreeMap::from([("R1".to_string(), &r), ("C1".to_string(), &c)]);
        let inputs = BTreeMap::from([("fc_hz".to_string(), Unit::Hertz)]);
        let compiled = rules(RC).compile(&inputs, &p, true).unwrap();
        let out = compiled.solve(&BTreeMap::from([("fc_hz".to_string(), 1e3)])).unwrap();
        assert_eq!(out.values["R1"]["resistance"], 16e3);
        assert_eq!(out.values["C1"]["capacitance"], 10e-9);
    }

    #[test]
    fn errors_are_listed() {
        let (r, c) = parts();
        let p = BTreeMap::from([("R1".to_string(), &r), ("C1".to_string(), &c)]);
        let inputs = BTreeMap::from([("fc_hz".to_string(), Unit::Hertz)]);
        let bad = rules(
            "solve: {tau: '1/(2*pi*fc_hz)', C1: 'search(E12, 100p, 10u)', R1: 'nearest(E24, tau*C1)', x: 'fc_hz + tau'}\nestimates: {q: 'R1'}",
        );
        let errs = bad.compile(&inputs, &p, true).unwrap_err();
        assert!(errs[0].starts_with("step R1: gives"), "{errs:?}");
        assert!(errs[1].starts_with("step x: cannot add"), "{errs:?}");
        assert!(errs[2].starts_with("estimate q: no target"), "{errs:?}");
        assert_eq!(errs.len(), 3, "{errs:?}");
    }

    #[test]
    fn too_many_candidates_is_an_authoring_error() {
        let (r, c) = parts();
        let p = BTreeMap::from([("R1".to_string(), &r), ("C1".to_string(), &c)]);
        let wide = rules("solve: {C1: 'search(E24, 1p, 1)', R1: 'search(E24, 1m, 1T)'}\nestimates: {}");
        let compiled = wide.compile(&BTreeMap::new(), &p, true).unwrap();
        assert!(compiled.solve(&BTreeMap::new()).unwrap_err().contains("more than"));
    }
}
