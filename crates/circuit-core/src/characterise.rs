//! Contracts and characterisation (LLD §17 D10). A component's contract is a bench (registry
//! parts around the component, `DUT`, and the nets joining them) and checks on the bench's nets,
//! each target an expression of the component's params. Characterisation runs the bench at points
//! spread over the params' ranges (as [`crate::template::verify_points`] does for templates):
//! [`benches`] compiles one netlist per point, the caller simulates them (WASM in the browser,
//! the sim runner on the server, with identical results), and [`report`] reads the results into
//! a report keyed by (definition hash, registry version, ngspice version). The report is derived
//! data: it is never stored in the IR, and the server recomputes it rather than trust a client.

use std::collections::{BTreeMap, BTreeSet};

use indexmap::IndexMap;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::apply::{apply_ops, refdes_prefix, valid_net_id};
use crate::error::{ErrorCode as E, OpError};
use crate::expr::{self, Dialect, Dim, Scope, Ty};
use crate::ir::{Analysis, Circuit, GND, NetKind, PartId, PinRef};
use crate::library::{self, ComponentDef, CompositePart};
use crate::ops::{AnalysisSet, Author, LibAdd, NetConnect, Op, PartAdd};
use crate::registry::{Category, PinType, Registry};
use crate::spice::{CompileOpts, EnvelopeResult, MeasDef, Netlist, compile, evaluate_envelope, node_name};
use crate::template::checks::{Need, Sim, emit_checks, merge_analyses, need};
use crate::template::{CheckDef, CheckResult, Domain, Scale, evaluate_checks};
use crate::units::{Quantity, Unit, parse_quantity};

/// The component's refdes in a bench's nets: `DUT.out`.
pub const DUT: &str = "DUT";
/// Checks in one contract.
pub const MAX_CHECKS: usize = 32;

#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Contract {
    pub bench: Bench,
    /// Checks on the bench's nets, each with a `target` expression of the component's params.
    pub checks: Vec<CheckDef>,
}

#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Bench {
    /// Registry parts around the component: sources, loads, feedback. Values may be expressions
    /// of the component's params.
    #[serde(default)]
    pub parts: IndexMap<String, CompositePart>,
    /// Net -> pins; `DUT.<pin>` is the component's pin, `GND` is ground.
    pub nets: IndexMap<String, Vec<PinRef>>,
    /// Analyses the checks do not imply (a DC sweep for `dc_transfer`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub analyses: Vec<Analysis>,
}

struct Params<'a>(&'a crate::registry::PartDef);

impl Scope for Params<'_> {
    fn var(&self, name: &str) -> Option<Ty> {
        self.0.params.get(name).map(|p| Ty::Dim(Dim::of(p.unit)))
    }
}

/// A bench value: a number with or without its unit, or an expression of the component's params.
fn bench_value(text: &str, unit: Unit, params: &BTreeMap<String, f64>) -> Result<f64, String> {
    match parse_quantity(text, unit) {
        Ok(q) => Ok(q.si),
        Err(_) => {
            let e = expr::parse(text, Dialect::Law).map_err(|e| e.message)?;
            Ok(expr::eval(&e, params).map_err(|e| e.message)?.value)
        }
    }
}

/// Every problem with a contract, against its definition.
pub fn check_contract(
    c: &Contract,
    def: &ComponentDef,
    reg: &Registry,
    _lib: &IndexMap<PartId, ComponentDef>,
) -> Vec<String> {
    let mut errs = Vec::new();
    let part = &def.part;
    let scope = Params(part);
    for (local, cp) in &c.bench.parts {
        if local == DUT {
            errs.push(format!("{DUT} is the component; name bench parts otherwise"));
            continue;
        }
        let Some(pd) = reg.part(&cp.part) else {
            errs.push(format!("bench part {local}: {} is not a registry part", cp.part));
            continue;
        };
        if refdes_prefix(local).as_deref() != Some(pd.refdes_prefix().as_str()) {
            errs.push(format!("bench part {local}: a {} needs a refdes like {}1", cp.part, pd.refdes_prefix()));
        }
        for (k, v) in &cp.params {
            let Some(pdef) = pd.params.get(k) else {
                errs.push(format!("bench part {local}: {} has no param {k}", cp.part));
                continue;
            };
            if parse_quantity(v, pdef.unit).is_ok() {
                continue;
            }
            match expr::parse(v, Dialect::Law).and_then(|e| expr::check(&e, Dialect::Law, &scope)) {
                Ok(t) if t.fits(pdef.unit) => {}
                Ok(t) => errs.push(format!("bench part {local}: {k} = {v} gives {}", t.dim().describe())),
                Err(e) => errs.push(format!("bench part {local}: {k} = {v}: {e}")),
            }
        }
    }
    let mut seen: BTreeSet<&PinRef> = BTreeSet::new();
    for (name, pins) in &c.bench.nets {
        if name != GND && !valid_net_id(name) {
            errs.push(format!("bench net {name}: not a valid net name"));
        }
        for p in pins {
            let known = if p.refdes == DUT {
                part.pin(&p.pin).is_some()
            } else {
                c.bench
                    .parts
                    .get(&p.refdes)
                    .and_then(|cp| reg.part(&cp.part))
                    .is_some_and(|pd| pd.pin(&p.pin).is_some())
            };
            if !known {
                errs.push(format!("bench net {name}: no pin {p}"));
            }
            if !seen.insert(p) {
                errs.push(format!("bench: {p} is on two nets"));
            }
        }
    }
    for p in part.pins.iter().filter(|p| p.kind != PinType::Nc) {
        if !seen.contains(&PinRef::new(DUT, &p.name)) {
            errs.push(format!("bench: {DUT}.{} is on no net", p.name));
        }
    }
    for a in &c.bench.analyses {
        if let Analysis::Dc { source, .. } = a
            && !c.bench.parts.get(source).and_then(|cp| reg.part(&cp.part)).is_some_and(|pd| pd.category == Category::V)
        {
            errs.push(format!("bench: the DC sweep source {source} is not a voltage source of the bench"));
        }
    }
    if c.checks.is_empty() || c.checks.len() > MAX_CHECKS {
        errs.push(format!("a contract has 1 to {MAX_CHECKS} checks"));
    }
    let mut names = BTreeSet::new();
    for ch in &c.checks {
        if !names.insert(ch.name.as_str()) {
            errs.push(format!("check {}: two checks share the name", ch.name));
        }
        errs.extend(ch.problems(&|n| c.bench.nets.contains_key(n)));
        match &ch.target {
            None => errs.push(format!("check {}: needs a target (an expression of the params)", ch.name)),
            Some(t) => match expr::parse(t, Dialect::Law).and_then(|e| expr::check(&e, Dialect::Law, &scope)) {
                Ok(ty) if ty.fits(ch.kind.unit()) => {}
                Ok(ty) => errs.push(format!(
                    "check {}: the target gives {}, but a {:?} check measures {}",
                    ch.name,
                    ty.dim().describe(),
                    ch.kind,
                    Dim::of(ch.kind.unit()).describe()
                )),
                Err(e) => errs.push(format!("check {}: target: {e}", ch.name)),
            },
        }
    }
    errs
}

/// Characterisation points: up to 5 param sets spread over every param with a range, as
/// template verification spreads targets (corners and the centre; log for ranges over a decade),
/// rounded to 3 significant digits. A component without ranged params has one point, its defaults.
pub fn points(part: &crate::registry::PartDef) -> Result<Vec<BTreeMap<String, f64>>, String> {
    const CORNERS: [(f64, f64); 5] = [(0.0, 0.0), (1.0, 1.0), (0.0, 1.0), (1.0, 0.0), (0.5, 0.5)];
    let defaults = library::param_values(part, &|_| None)?;
    let ranged: Vec<(&String, f64, f64, Scale)> = part
        .params
        .iter()
        .filter_map(|(k, p)| {
            let (lo, hi) = (p.min?, p.max?);
            (lo < hi).then(|| (k, lo, hi, if lo > 0.0 && hi / lo >= 10.0 { Scale::Log } else { Scale::Lin }))
        })
        .collect();
    if ranged.is_empty() {
        return Ok(vec![defaults]);
    }
    let dims = ranged.len();
    let mut out: Vec<BTreeMap<String, f64>> = Vec::new();
    for (k, corner) in CORNERS.iter().enumerate() {
        let frac = |d: usize| match (dims, d) {
            (1, _) => k as f64 / 4.0,
            (_, 0) => corner.0,
            (_, 1) => corner.1,
            _ => ((k + 2 * d) % 5) as f64 / 4.0,
        };
        let mut p = defaults.clone();
        for (d, (name, lo, hi, scale)) in ranged.iter().enumerate() {
            let x = frac(d);
            let v = match scale {
                Scale::Lin => lo + x * (hi - lo),
                Scale::Log => (lo.ln() + x * (hi / lo).ln()).exp(),
            };
            let v: f64 = format!("{v:.2e}").parse().expect("{:.2e} re-parses");
            p.insert((*name).clone(), v.clamp(*lo, *hi));
        }
        if !out.contains(&p) {
            out.push(p);
        }
    }
    Ok(out)
}

/// One characterisation point: its params and the bench netlist, or why it cannot be built.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
pub struct BenchRun {
    /// Param values as text, for reading (`10kΩ`).
    pub params: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub netlist: Option<Netlist>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// What a simulator returned for one bench: its `.meas` values, or the failure.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, Default, PartialEq)]
pub struct SimOutcome {
    #[serde(default)]
    pub meas: BTreeMap<String, f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Trust {
    /// Passed its own contract at every point in our simulator.
    Characterised,
    /// Not characterised, or a check failed.
    Draft,
}

#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
pub struct PointReport {
    pub params: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub netlist_hash: Option<String>,
    pub checks: Vec<CheckResult>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub envelope: Vec<EnvelopeResult>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    pub pass: bool,
}

/// A characterisation report (LLD §17 D10), derived and keyed by what it depends on.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
pub struct Characterisation {
    pub component: PartId,
    /// [`library::def_hash`] of the component and everything it uses.
    pub def_hash: String,
    pub registry_version: String,
    pub ngspice: String,
    pub trust: Trust,
    pub points: Vec<PointReport>,
}

/// Library ids in an order where each comes after everything it uses.
fn in_dependency_order<'a>(lib: &'a IndexMap<PartId, ComponentDef>, id: &'a str) -> Vec<&'a str> {
    let all = library::closure(lib, [id]);
    let mut done: Vec<&str> = Vec::new();
    while done.len() < all.len() {
        for d in &all {
            if !done.contains(d) && library::direct_uses(&lib[*d]).iter().all(|u| done.contains(u)) {
                done.push(d);
            }
        }
    }
    done
}

/// The bench netlist of `id` at each characterisation point.
pub fn benches(reg: &Registry, lib: &IndexMap<PartId, ComponentDef>, id: &str) -> Result<Vec<BenchRun>, OpError> {
    let def = lib.get(id).ok_or_else(|| OpError::new(E::ComponentNotFound, format!("no component {id}")))?;
    let contract = def
        .contract
        .as_ref()
        .ok_or_else(|| OpError::new(E::ComponentInvalid, format!("{id} has no contract to characterise")))?;
    let pts = points(&def.part).map_err(|e| OpError::new(E::ComponentInvalid, e))?;
    Ok(pts
        .iter()
        .map(|values| {
            let params =
                values.iter().map(|(k, v)| (k.clone(), Quantity::new(*v, def.part.params[k].unit).display)).collect();
            match bench(reg, lib, id, def, contract, values) {
                Ok(n) => BenchRun { params, netlist: Some(n), error: None },
                Err(e) => BenchRun { params, netlist: None, error: Some(e) },
            }
        })
        .collect())
}

fn bench(
    reg: &Registry,
    lib: &IndexMap<PartId, ComponentDef>,
    id: &str,
    def: &ComponentDef,
    contract: &Contract,
    values: &BTreeMap<String, f64>,
) -> Result<Netlist, String> {
    let prefix = def.part.refdes_prefix();
    let dut =
        (1..).map(|k| format!("{prefix}{k}")).find(|r| !contract.bench.parts.contains_key(r)).expect("a free refdes");
    let mut ops: Vec<Op> = in_dependency_order(lib, id)
        .into_iter()
        .map(|d| Op::LibAdd(LibAdd { def: Box::new(lib[d].clone()) }))
        .collect();
    let shown = |unit: Unit, v: f64| Quantity::new(v, unit).display;
    ops.push(Op::PartAdd(PartAdd {
        refdes: dut.clone(),
        part: id.to_string(),
        params: values.iter().map(|(k, v)| (k.clone(), shown(def.part.params[k].unit, *v))).collect(),
        block: None,
        origin: None,
    }));
    for (local, cp) in &contract.bench.parts {
        let pd = reg.part(&cp.part).ok_or_else(|| format!("{} is not a registry part", cp.part))?;
        let mut params = BTreeMap::new();
        for (k, text) in &cp.params {
            let unit = pd.params[k].unit;
            let v = bench_value(text, unit, values).map_err(|e| format!("bench part {local}: {k}: {e}"))?;
            params.insert(k.clone(), shown(unit, v));
        }
        ops.push(Op::PartAdd(PartAdd {
            refdes: local.clone(),
            part: cp.part.clone(),
            params,
            block: None,
            origin: None,
        }));
    }
    for (net, pins) in &contract.bench.nets {
        let pins = pins.iter().map(|p| if p.refdes == DUT { PinRef::new(&dut, &p.pin) } else { p.clone() }).collect();
        let kind = (net == GND).then_some(NetKind::Ground);
        ops.push(Op::NetConnect(NetConnect { net: net.clone(), pins, kind, label: None }));
    }

    // Analyses: the bench's own, widened to what the checks need, plus the editor's default
    // transient when a time-domain check would otherwise have none.
    let targets: Vec<(&CheckDef, f64)> = contract
        .checks
        .iter()
        .map(|ch| {
            let t = ch.target.as_deref().unwrap_or("0");
            let e = expr::parse(t, Dialect::Law).map_err(|e| format!("check {}: {e}", ch.name))?;
            let v = expr::eval(&e, values).map_err(|e| format!("check {}: {e}", ch.name))?.value;
            Ok((ch, v))
        })
        .collect::<Result<_, String>>()?;
    let needs: Vec<Need> = targets.iter().map(|(ch, t)| need(ch, *t)).collect();
    let mut analyses =
        merge_analyses(&contract.bench.analyses, needs).unwrap_or_else(|| contract.bench.analyses.clone());
    let c0 = Circuit::new(reg.version.clone());
    let (mut c, _) = apply_ops(&c0, reg, &ops, Author::User, None).map_err(|e| e.message)?;
    if targets.iter().any(|(ch, _)| ch.kind.domain() == Domain::Tran)
        && !analyses.iter().any(|a| matches!(a, Analysis::Tran { .. }))
    {
        let tran = crate::spice::interactive_analyses(&c, reg)
            .into_iter()
            .find(|a| matches!(a, Analysis::Tran { .. }))
            .expect("the editor's set has a transient");
        analyses.push(tran);
    }
    let (next, _) =
        apply_ops(&c, reg, &[Op::AnalysisSet(AnalysisSet { analyses: analyses.clone() })], Author::User, None)
            .map_err(|e| e.message)?;
    c = next;
    let opts = CompileOpts { shunt_floating: true, analyses: Some(analyses.clone()), interactive: false };
    let mut n = compile(&c, reg, &opts).map_err(|e| e.message)?;

    let sim = Sim::new(&c, reg, &analyses);
    let mut cards = Vec::new();
    let mut defs = Vec::new();
    let node = |net: &str| {
        if c.nets.contains_key(net) { Ok(node_name(net)) } else { Err(format!("bench net {net} is not in the bench")) }
    };
    emit_checks(&sim, "dut", targets.iter().map(|(ch, t)| (*ch, *t, ch.tol_pct)), &node, &mut cards, &mut defs);
    let end = n.text.rfind(".end\n").expect("a netlist ends with .end");
    let lines: String = cards.iter().map(|m: &MeasDef| format!("{}\n", m.line)).collect();
    n.text.insert_str(end, &lines);
    n.hash = crate::spice::text_hash(&n.text);
    n.meas.extend(cards);
    n.checks.extend(defs);
    Ok(n)
}

/// Read simulation outcomes (one per bench, in order) into a report.
pub fn report(
    reg: &Registry,
    lib: &IndexMap<PartId, ComponentDef>,
    id: &str,
    ngspice: &str,
    runs: &[BenchRun],
    outcomes: &[SimOutcome],
) -> Result<Characterisation, OpError> {
    if !lib.contains_key(id) {
        return Err(OpError::new(E::ComponentNotFound, format!("no component {id}")));
    }
    if runs.len() != outcomes.len() {
        return Err(OpError::new(E::BadValue, format!("{} benches but {} outcomes", runs.len(), outcomes.len())));
    }
    let mut points = Vec::new();
    for (run, out) in runs.iter().zip(outcomes) {
        let mut p = PointReport {
            params: run.params.clone(),
            netlist_hash: run.netlist.as_ref().map(|n| n.hash.clone()),
            checks: Vec::new(),
            envelope: Vec::new(),
            error: run.error.clone().or_else(|| out.error.clone()),
            pass: false,
        };
        if let (Some(n), None) = (&run.netlist, &p.error) {
            p.checks = evaluate_checks(&n.checks, &out.meas);
            p.envelope = evaluate_envelope(&n.envelope, &out.meas);
            p.pass = p.checks.iter().all(|c| c.pass) && p.envelope.iter().all(|e| e.pass);
        }
        points.push(p);
    }
    let trust = if !points.is_empty() && points.iter().all(|p| p.pass) { Trust::Characterised } else { Trust::Draft };
    Ok(Characterisation {
        component: id.to_string(),
        def_hash: library::def_hash(lib, id),
        registry_version: reg.version.clone(),
        ngspice: ngspice.to_string(),
        trust,
        points,
    })
}
