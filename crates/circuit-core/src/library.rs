//! The project library (LLD §17 D1, D2, D13): the component definitions a circuit uses, embedded
//! in the circuit so a project stays a pure fold of its op log, with no library fetch on any
//! runtime. A definition is a part from the outside (pins, a refdes prefix, params, limits, a
//! symbol) and a model inside (D3): a composite of parts, behavioral laws, or an imported SPICE
//! model. An instance of it is an ordinary `PartInstance` whose `part` is the definition's id
//! (`lib:<name>`), compiled to an `X` line and one `.subckt` per distinct set of param values.
//!
//! This module only reads: `lib.add`, `lib.update` and `lib.remove` in `apply` change the library,
//! after [`prepare`] has checked a definition and filled in what the core computes.

use std::collections::{BTreeMap, BTreeSet};

use indexmap::IndexMap;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::apply::{refdes_prefix, valid_net_id};
use crate::behavioral::{self, Behavioral, Envelope};
use crate::characterise::Contract;
use crate::error::{ErrorCode as E, OpError};
use crate::expr::{self, Dialect, Dim, Scope, Ty};
use crate::ir::{Circuit, GND, Origin, PartId, PartInstance, PinRef, refdes_sort_key};
use crate::registry::{Category, PartDef, PinType, Registry};
use crate::rules::{Rules, ValueRules};
use crate::spice_import::{self, SpiceModel};
use crate::units::{Quantity, parse_quantity, spice_number};

/// Library ids are `lib:` and a lowercase name, so they never clash with a registry part.
pub const LIB_PREFIX: &str = "lib:";
/// Nesting depth of definitions inside definitions.
pub const MAX_DEPTH: usize = 8;
/// Parts and nets inside one definition (the circuit's own limits, LLD §1, per level).
pub const MAX_DEF_PARTS: usize = 300;
pub const MAX_DEF_NETS: usize = 400;
/// Definitions in one project library.
pub const MAX_LIBRARY: usize = 64;
/// Params of one definition.
pub const MAX_PARAMS: usize = 32;
/// Refdes prefixes a library component may take (it compiles to an `X` line, so any of these is
/// safe in SPICE).
pub const LIBRARY_PREFIXES: [&str; 12] = ["X", "U", "XU", "XA", "IC", "TP", "SW", "RV", "K", "F", "Y", "BT"];
/// Names an expression gives another meaning, so no param may take them.
const RESERVED_NAMES: [&str; 8] = ["v", "i", "time", "pi", "temper", "E6", "E12", "E24"];

#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ComponentDef {
    /// The component as a part: id (`lib:<name>`), category `X`, title, refdes `prefix`, pins,
    /// params (each with a unit, a default and a range), limits and symbol (`auto`, a box the
    /// editor draws from the pins, or a registry symbol whose anchors match the pins). It has no
    /// `spice` line: its model is.
    pub part: PartDef,
    pub version: u32,
    pub model: Model,
    pub provenance: Provenance,
    /// A composite's part values from its params (LLD §17 D5), searched over the E-series.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rules: Option<ValueRules>,
    /// What the component promises and the bench that tests it (D10): characterisation runs it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub contract: Option<Contract>,
}

/// What the component does inside (LLD §17 D3).
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Model {
    Composite(Composite),
    /// Laws compiled to B-sources, R, C and I ([`crate::behavioral`]).
    Behavioral(Behavioral),
    /// A vendor `.subckt`, parsed by an allow-list and re-emitted ([`crate::spice_import`]).
    Spice(SpiceModel),
}

/// A sub-circuit of registry parts and other library components.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
pub struct Composite {
    /// Local refdes (`R1`, `U1`, scoped to the definition) -> part.
    pub parts: IndexMap<String, CompositePart>,
    /// Net -> pins. A net named after one of the component's pins is that pin; `GND` is the
    /// circuit's ground; every other net is internal.
    pub nets: IndexMap<String, Vec<PinRef>>,
}

#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct CompositePart {
    pub part: PartId,
    /// Values as written ("10k"), or, in a component with params, expressions of them
    /// ("10k*gain"); a missing param takes the part's default, unless the rules set it.
    #[serde(default)]
    pub params: BTreeMap<String, String>,
}

/// Who made a definition (LLD §17 D10: trust levels read it).
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Provenance {
    User,
    Llm { job_id: String },
    Import { source: String, licence: String },
}

pub const AUTO_SYMBOL: &str = "auto";

/// The part behind an instance's `part` id: the registry's, or the circuit library's.
pub fn part_def<'a>(reg: &'a Registry, c: &'a Circuit, id: &str) -> Option<&'a PartDef> {
    lookup(reg, &c.library, id)
}

pub fn lookup<'a>(reg: &'a Registry, lib: &'a IndexMap<PartId, ComponentDef>, id: &str) -> Option<&'a PartDef> {
    if id.starts_with(LIB_PREFIX) { lib.get(id).map(|d| &d.part) } else { reg.part(id) }
}

pub fn is_library_id(id: &str) -> bool {
    id.strip_prefix(LIB_PREFIX).is_some_and(|n| {
        (1..=48).contains(&n.len())
            && n.as_bytes()[0].is_ascii_lowercase()
            && n.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
    })
}

/// Pin groups of a part that conduct DC between each other, and whether each group also reaches
/// ground inside the part. Registry parts follow their category (as ERC003 always has); library
/// components follow the `dc_group` [`prepare`] computed.
pub(crate) fn conducting(def: &PartDef) -> Vec<(Vec<&str>, bool)> {
    let pins = |keep: &dyn Fn(PinType) -> bool| -> Vec<&str> {
        def.pins.iter().filter(|p| keep(p.kind)).map(|p| p.name.as_str()).collect()
    };
    match def.category {
        Category::R | Category::L | Category::V | Category::D | Category::Q => {
            vec![(pins(&|t| t != PinType::Nc), false)]
        }
        Category::U => vec![(pins(&|t| !matches!(t, PinType::Input | PinType::Nc)), false)],
        Category::C | Category::J => Vec::new(),
        Category::X => {
            let mut groups: BTreeMap<u16, Vec<&str>> = BTreeMap::new();
            for p in &def.pins {
                if let Some(g) = p.dc_group {
                    groups.entry(g).or_default().push(&p.name);
                }
            }
            groups.into_iter().map(|(g, pins)| (pins, g == 0)).collect()
        }
    }
}

/// Union-find over a definition's net names.
struct Dsu {
    parent: Vec<usize>,
}

impl Dsu {
    fn new(n: usize) -> Dsu {
        Dsu { parent: (0..n).collect() }
    }
    fn find(&mut self, x: usize) -> usize {
        let p = self.parent[x];
        if p == x {
            return x;
        }
        let r = self.find(p);
        self.parent[x] = r;
        r
    }
    fn union(&mut self, a: usize, b: usize) {
        let (ra, rb) = (self.find(a), self.find(b));
        if ra != rb {
            let (lo, hi) = if ra < rb { (ra, rb) } else { (rb, ra) };
            self.parent[hi] = lo;
        }
    }
}

/// Library definitions `def` uses directly (only a composite contains other components).
pub fn direct_uses(def: &ComponentDef) -> BTreeSet<&str> {
    match &def.model {
        Model::Composite(m) => {
            m.parts.values().filter(|p| p.part.starts_with(LIB_PREFIX)).map(|p| p.part.as_str()).collect()
        }
        _ => BTreeSet::new(),
    }
}

/// `id` and every library definition it uses, transitively.
pub fn closure<'a>(
    lib: &'a IndexMap<PartId, ComponentDef>,
    ids: impl IntoIterator<Item = &'a str>,
) -> BTreeSet<&'a str> {
    let mut seen = BTreeSet::new();
    let mut todo: Vec<&str> = ids.into_iter().collect();
    while let Some(id) = todo.pop() {
        if let Some((k, d)) = lib.get_key_value(id)
            && seen.insert(k.as_str())
        {
            todo.extend(direct_uses(d));
        }
    }
    seen
}

fn depth(lib: &IndexMap<PartId, ComponentDef>, id: &str) -> usize {
    match lib.get(id) {
        Some(d) => 1 + direct_uses(d).into_iter().map(|u| depth(lib, u)).max().unwrap_or(0),
        None => 0,
    }
}

/// The content hash of a definition and everything it uses (LLD §17 D1, D10): lowercase hex
/// sha256 of each definition's JSON, in id order.
pub fn def_hash(lib: &IndexMap<PartId, ComponentDef>, id: &str) -> String {
    let mut h = Sha256::new();
    for k in closure(lib, [id]) {
        h.update(k.as_bytes());
        h.update([0]);
        h.update(serde_json::to_string(&lib[k]).expect("a definition serializes").as_bytes());
        h.update([0]);
    }
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

fn invalid(problems: Vec<String>) -> OpError {
    OpError::new(E::ComponentInvalid, problems.join("; "))
}

/// Every param of `part` in SI units: `given` where it has one, else the param's default.
pub fn param_values(part: &PartDef, given: &dyn Fn(&str) -> Option<f64>) -> Result<BTreeMap<String, f64>, String> {
    part.params
        .iter()
        .map(|(k, p)| {
            let v = match given(k) {
                Some(v) => v,
                None => parse_quantity(&p.default, p.unit).map_err(|e| format!("param {k}: {e}"))?.si,
            };
            Ok((k.clone(), v))
        })
        .collect()
}

/// Param values of an instance in the circuit.
pub fn instance_values(part: &PartDef, inst: &PartInstance) -> Result<BTreeMap<String, f64>, String> {
    param_values(part, &|k| inst.params.get(k).map(|q| q.si))
}

/// The `.subckt` name of a definition without params: `lib_<name>`.
pub fn subckt_name(id: &str) -> String {
    format!("lib_{}", id.strip_prefix(LIB_PREFIX).unwrap_or(id))
}

/// The `.subckt` name of one use: `lib_<name>` without params; with params, followed by the
/// first 8 hex digits of a hash of the values, so each distinct value set is its own sub-circuit
/// with its values written in (LLD §17, P5.0: no user value goes through numparam).
pub fn subckt_name_for(id: &str, part: &PartDef, values: &BTreeMap<String, f64>) -> String {
    if part.params.is_empty() {
        return subckt_name(id);
    }
    let mut h = Sha256::new();
    for (k, v) in values {
        h.update(format!("{k}={};", spice_number(*v)).as_bytes());
    }
    let hex: String = h.finalize().iter().take(4).map(|b| format!("{b:02x}")).collect();
    format!("{}_{hex}", subckt_name(id))
}

/// Param names, units and scope for the expressions of a definition.
struct Params<'a>(&'a PartDef);

impl Scope for Params<'_> {
    fn var(&self, name: &str) -> Option<Ty> {
        self.0.params.get(name).map(|p| Ty::Dim(Dim::of(p.unit)))
    }
}

fn param_name_ok(name: &str) -> bool {
    let b = name.as_bytes();
    !b.is_empty()
        && b.len() <= 32
        && b[0].is_ascii_lowercase()
        && b.iter().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'_')
        && !RESERVED_NAMES.contains(&name)
}

/// Check `def` against the registry and the library it joins (`lib`, without `def` itself), and
/// return it with what the core computes filled in: each pin's `dc_group`. Every problem is
/// listed at once, so an author (a learner or a model) fixes them in one round.
pub fn prepare(
    def: &ComponentDef,
    reg: &Registry,
    lib: &IndexMap<PartId, ComponentDef>,
) -> Result<ComponentDef, OpError> {
    let mut errs = Vec::new();
    let part = &def.part;
    if !is_library_id(&part.id) {
        errs.push(format!(
            "id \"{}\" must be {LIB_PREFIX} and a lowercase [a-z0-9_] name of up to 48 characters",
            part.id
        ));
    }
    if part.category != Category::X {
        errs.push("a library component has category X".into());
    }
    let prefix = part.refdes_prefix();
    if !LIBRARY_PREFIXES.contains(&prefix.as_str()) {
        errs.push(format!("prefix {prefix} is not one of {}", LIBRARY_PREFIXES.join(", ")));
    }
    if part.spice.is_some() || part.dc_param.is_some() {
        errs.push("a library component has no spice line or dc_param: its model is its netlist".into());
    }
    if !part.units.is_empty() || part.pins.iter().any(|p| p.unit.is_some()) {
        errs.push("library components have no units".into());
    }
    if part.params.len() > MAX_PARAMS {
        errs.push(format!("at most {MAX_PARAMS} params"));
    }
    for name in part.params.keys() {
        if !param_name_ok(name) {
            errs.push(format!(
                "param \"{name}\" must be a lowercase [a-z0-9_] name, and not one of {}",
                RESERVED_NAMES.join(", ")
            ));
        }
    }
    if part.symbol != AUTO_SYMBOL {
        errs.extend(part.check_symbol(&reg.symbols));
    }
    errs.extend(part.validate_common());
    let mut lower = BTreeSet::new();
    for p in &part.pins {
        if !lower.insert(p.name.to_ascii_lowercase()) {
            errs.push(format!("pins {} differ only in case (SPICE node names do not)", p.name));
        }
    }
    for (k, v) in &part.limits {
        if !v.is_finite() {
            errs.push(format!("limit {k} must be finite"));
        }
    }
    if def.rules.is_some() && !matches!(def.model, Model::Composite(_)) {
        errs.push("rules set the values of parts: only a composite has parts".into());
    }
    if !errs.is_empty() {
        return Err(invalid(errs));
    }

    let groups = match &def.model {
        Model::Composite(m) => composite_groups(def, m, reg, lib)?,
        Model::Behavioral(b) => {
            let laws = behavioral::parse(b, part).map_err(invalid)?;
            let joins = behavioral::conducting(&laws);
            groups_from_joins(part, &joins)
        }
        Model::Spice(s) => {
            let pins: BTreeSet<&str> = part.pins.iter().map(|p| p.name.as_str()).collect();
            let mut errs = Vec::new();
            for pin in s.pins.keys() {
                if !pins.contains(pin.as_str()) {
                    errs.push(format!("pin map: the component has no pin {pin}"));
                }
            }
            for pin in &pins {
                if !s.pins.contains_key(*pin) {
                    errs.push(format!("pin map: pin {pin} is mapped to no port"));
                }
            }
            match spice_import::defaults(s) {
                Ok(d) => {
                    for k in part.params.keys() {
                        if !d.iter().any(|x| x.eq_ignore_ascii_case(k)) {
                            errs.push(format!("param {k} sets no PARAMS: default of {}", s.subckt));
                        }
                    }
                }
                Err(e) => errs.push(e),
            }
            if !errs.is_empty() {
                return Err(invalid(errs));
            }
            spice_import::conducting(s).map_err(|e| invalid(vec![e]))?
        }
    };
    if let Some(c) = &def.contract {
        let errs = crate::characterise::check_contract(c, def, reg, lib);
        if !errs.is_empty() {
            return Err(invalid(errs.into_iter().map(|e| format!("contract: {e}")).collect()));
        }
    }
    // It must compile at its default params.
    let defaults = param_values(part, &|_| None).map_err(|e| invalid(vec![e]))?;
    let name = subckt_name_for(&part.id, part, &defaults);
    body(def, reg, lib, &defaults, &name).map_err(|e| invalid(vec![format!("at the default params: {e}")]))?;

    let mut out = def.clone();
    number_groups(&mut out.part, &groups);
    Ok(out)
}

/// Pin groups joined by pairs (`None`: ground).
fn groups_from_joins(part: &PartDef, joins: &[(String, Option<String>)]) -> Vec<(Vec<String>, bool)> {
    let names: Vec<&str> = part.pins.iter().map(|p| p.name.as_str()).collect();
    let index: BTreeMap<&str, usize> = names.iter().enumerate().map(|(i, n)| (*n, i)).collect();
    let gnd = names.len();
    let mut dsu = Dsu::new(names.len() + 1);
    for (a, b) in joins {
        let ia = index[a.as_str()];
        let ib = b.as_deref().map_or(gnd, |b| index[b]);
        dsu.union(ia, ib);
    }
    let groot = dsu.find(gnd);
    let mut by_root: BTreeMap<usize, Vec<String>> = BTreeMap::new();
    for (i, n) in names.iter().enumerate() {
        by_root.entry(dsu.find(i)).or_default().push(n.to_string());
    }
    by_root.into_iter().map(|(r, pins)| (pins, r == groot)).collect()
}

/// Group numbers in pin order: 0 for pins that reach ground, 1, 2, … for groups of two or more
/// pins, none for a pin that conducts to nothing.
fn number_groups(part: &mut PartDef, groups: &[(Vec<String>, bool)]) {
    let mut numbers: BTreeMap<usize, u16> = BTreeMap::new();
    for p in part.pins.iter_mut() {
        let g = groups.iter().position(|(pins, _)| pins.contains(&p.name));
        p.dc_group = match g {
            Some(k) if groups[k].1 => Some(0),
            Some(k) if groups[k].0.len() > 1 => {
                let next = numbers.len() as u16 + 1;
                Some(*numbers.entry(k).or_insert(next))
            }
            _ => None,
        };
    }
}

/// A composite's checks: parts, values, nets, nesting, rules; then its DC groups.
fn composite_groups(
    def: &ComponentDef,
    m: &Composite,
    reg: &Registry,
    lib: &IndexMap<PartId, ComponentDef>,
) -> Result<Vec<(Vec<String>, bool)>, OpError> {
    let part = &def.part;
    let mut errs = Vec::new();
    if m.parts.is_empty() {
        errs.push("a composite needs at least one part".into());
    }
    if m.parts.len() > MAX_DEF_PARTS || m.nets.len() > MAX_DEF_NETS {
        errs.push(format!("at most {MAX_DEF_PARTS} parts and {MAX_DEF_NETS} nets per definition"));
    }
    // Parts: each a registry part or an earlier library component, with a matching local refdes.
    let mut defs: BTreeMap<&str, &PartDef> = BTreeMap::new();
    for (local, cp) in &m.parts {
        let Some(pd) = lookup(reg, lib, &cp.part) else {
            let what = if cp.part.starts_with(LIB_PREFIX) { "the project library" } else { "the registry" };
            errs.push(format!("part {local}: {} is not in {what}", cp.part));
            continue;
        };
        if cp.part == part.id {
            errs.push(format!("part {local}: a component cannot contain itself"));
            continue;
        }
        if cp.part.starts_with(LIB_PREFIX) && closure(lib, [cp.part.as_str()]).contains(part.id.as_str()) {
            errs.push(format!("part {local}: {} uses {}, which would make a loop", cp.part, part.id));
            continue;
        }
        if refdes_prefix(local).as_deref() != Some(pd.refdes_prefix().as_str()) {
            errs.push(format!("part {local}: a {} needs a refdes like {}1", cp.part, pd.refdes_prefix()));
        }
        for (k, v) in &cp.params {
            match pd.params.get(k) {
                None => errs.push(format!("part {local}: {} has no param {k}", cp.part)),
                Some(pdef) => match parse_quantity(v, pdef.unit) {
                    Ok(q) if pdef.min.is_some_and(|lo| q.si < lo) || pdef.max.is_some_and(|hi| q.si > hi) => {
                        errs.push(format!("part {local}: {k}={} is outside the part's range", q.display))
                    }
                    Ok(_) => {}
                    // In a component with params, a value may be an expression of them.
                    Err(e) if part.params.is_empty() => errs.push(format!("part {local}: {k}: {e}")),
                    Err(_) => {
                        match expr::parse(v, Dialect::Law).and_then(|x| expr::check(&x, Dialect::Law, &Params(part))) {
                            Ok(t) if t.fits(pdef.unit) => {}
                            Ok(t) => errs.push(format!(
                                "part {local}: {k} = {v} gives {}, but {k} is in {}",
                                t.dim().describe(),
                                Dim::of(pdef.unit).describe()
                            )),
                            Err(e) => errs.push(format!("part {local}: {k} = {v}: {e}")),
                        }
                    }
                },
            }
        }
        defs.insert(local, pd);
    }
    let uses = direct_uses(def);
    if 1 + uses.iter().map(|u| depth(lib, u)).max().unwrap_or(0) > MAX_DEPTH {
        errs.push(format!("components nest at most {MAX_DEPTH} deep"));
    }
    if let Some(rules) = &def.rules
        && let Err(e) = compile_rules(def, rules, reg, lib)
    {
        errs.extend(e.into_iter().map(|m| format!("rules: {m}")));
    }

    // Nets: pin nets, GND and internal nets; each pin of each part on exactly one.
    let pin_names: BTreeSet<&str> = part.pins.iter().map(|p| p.name.as_str()).collect();
    let mut net_lower = BTreeSet::new();
    let mut on_net: BTreeMap<&PinRef, &str> = BTreeMap::new();
    for (name, pins) in &m.nets {
        let is_port = pin_names.contains(name.as_str());
        if !is_port && name != GND && !valid_net_id(name) {
            errs.push(format!("net {name}: not a valid net name"));
        }
        if !net_lower.insert(name.to_ascii_lowercase()) {
            errs.push(format!("net {name}: two nets differ only in case"));
        }
        if pins.is_empty() || (!is_port && name != GND && pins.len() < 2) {
            errs.push(format!("net {name}: an internal net needs at least 2 pins"));
        }
        for p in pins {
            match defs.get(p.refdes.as_str()) {
                None if !m.parts.contains_key(&p.refdes) => {
                    errs.push(format!("net {name}: {p} names no part of the component"))
                }
                None => {}
                Some(pd) if pd.pin(&p.pin).is_none() => {
                    let names: Vec<&str> = pd.pins.iter().map(|q| q.name.as_str()).collect();
                    errs.push(format!("net {name}: {} has pins {}", p.refdes, names.join(" ")))
                }
                Some(_) => {
                    if let Some(other) = on_net.insert(p, name) {
                        errs.push(format!("{p} is on {other} and {name}"));
                    }
                }
            }
        }
    }
    for p in &part.pins {
        if !m.nets.contains_key(&p.name) {
            errs.push(format!("pin {} has no net inside the component", p.name));
        }
    }
    for (local, pd) in &defs {
        let on = |pin: &str| on_net.contains_key(&PinRef::new(*local, pin));
        for p in &pd.pins {
            let unit_unused = p.unit.as_ref().is_some_and(|u| !pd.unit_pins(u).any(|q| on(&q.name)));
            if p.kind != PinType::Nc && !unit_unused && !on(&p.name) {
                errs.push(format!("{local}.{} is not on any net", p.name));
            }
        }
    }
    if !errs.is_empty() {
        return Err(invalid(errs));
    }

    // DC groups: join the nets each part conducts between; a pin's group is its net's.
    let names: Vec<&str> = m.nets.keys().map(String::as_str).collect();
    let index: BTreeMap<&str, usize> = names.iter().enumerate().map(|(i, n)| (*n, i)).collect();
    let gnd = names.len(); // a node for ground, used or not
    let mut dsu = Dsu::new(names.len() + 1);
    if let Some(i) = index.get(GND) {
        dsu.union(*i, gnd);
    }
    for (local, pd) in &defs {
        for (group, to_ground) in conducting(pd) {
            let nets: Vec<usize> =
                group.iter().filter_map(|pin| on_net.get(&PinRef::new(*local, *pin)).map(|n| index[n])).collect();
            for w in nets.windows(2) {
                dsu.union(w[0], w[1]);
            }
            if to_ground && let Some(first) = nets.first() {
                dsu.union(*first, gnd);
            }
        }
    }
    let ground_root = dsu.find(gnd);
    let port_roots: BTreeSet<usize> = part.pins.iter().map(|p| dsu.find(index[p.name.as_str()])).collect();
    let mut floating = Vec::new();
    for (i, n) in names.iter().enumerate() {
        let r = dsu.find(i);
        if r != ground_root && !port_roots.contains(&r) {
            floating.push(*n);
        }
    }
    if !floating.is_empty() {
        return Err(invalid(vec![format!(
            "{} has no DC path to a pin of the component or to GND (the simulation would be singular)",
            floating.join(", ")
        )]));
    }
    let mut by_root: BTreeMap<usize, Vec<String>> = BTreeMap::new();
    for p in &part.pins {
        by_root.entry(dsu.find(index[p.name.as_str()])).or_default().push(p.name.clone());
    }
    Ok(by_root.into_iter().map(|(r, pins)| (pins, r == ground_root)).collect())
}

/// A composite's rules checked against its params and the parts whose values they set.
pub fn compile_rules(
    def: &ComponentDef,
    rules: &ValueRules,
    reg: &Registry,
    lib: &IndexMap<PartId, ComponentDef>,
) -> Result<Rules, Vec<String>> {
    let Model::Composite(m) = &def.model else { return Err(vec!["rules need a composite".into()]) };
    let inputs = def.part.params.iter().map(|(k, p)| (k.clone(), p.unit)).collect();
    let mut parts = BTreeMap::new();
    for (local, cp) in &m.parts {
        if let Some(pd) = lookup(reg, lib, &cp.part)
            && !pd.params.is_empty()
        {
            parts.insert(local.clone(), pd);
        }
    }
    // Parts the rules leave alone keep their written values.
    rules.compile(&inputs, &parts, false)
}

/// The values of every part param inside a composite at the component's param values: the
/// rules' choice, else the written value or expression, else the part's default.
pub fn composite_values(
    def: &ComponentDef,
    m: &Composite,
    reg: &Registry,
    lib: &IndexMap<PartId, ComponentDef>,
    values: &BTreeMap<String, f64>,
) -> Result<BTreeMap<String, BTreeMap<String, f64>>, String> {
    let solved = match &def.rules {
        Some(r) => compile_rules(def, r, reg, lib).map_err(|e| e.join("; "))?.solve(values)?.values,
        None => BTreeMap::new(),
    };
    let mut out = BTreeMap::new();
    for (local, cp) in &m.parts {
        let pd = lookup(reg, lib, &cp.part).ok_or_else(|| format!("{} is unknown", cp.part))?;
        let mut vals = BTreeMap::new();
        for (k, pdef) in &pd.params {
            let v = if let Some(v) = solved.get(local).and_then(|s| s.get(k)) {
                *v
            } else if let Some(text) = cp.params.get(k) {
                match parse_quantity(text, pdef.unit) {
                    Ok(q) => q.si,
                    Err(_) => {
                        let e = expr::parse(text, Dialect::Law).map_err(|e| format!("{local}.{k}: {e}"))?;
                        expr::eval(&e, values).map_err(|e| format!("{local}.{k}: {e}"))?.value
                    }
                }
            } else {
                parse_quantity(&pdef.default, pdef.unit).map_err(|e| format!("{local}.{k}: {e}"))?.si
            };
            if !v.is_finite() || pdef.min.is_some_and(|lo| v < lo) || pdef.max.is_some_and(|hi| v > hi) {
                let shown = crate::units::format_eng(v, pdef.unit);
                return Err(format!("{local}.{k} = {shown} is outside the part's range"));
            }
            vals.insert(k.clone(), v);
        }
        out.insert(local.clone(), vals);
    }
    Ok(out)
}

/// One `.subckt` of a definition at given param values.
#[derive(Clone, Debug, PartialEq)]
pub struct Body {
    /// Complete lines: model cards and helper sub-circuits first, then `.subckt name … .ends`.
    pub lines: Vec<String>,
    /// Primitive devices in this body, nested library components not included.
    pub leaf_devices: u64,
    /// Nested library components: (id, param values) per `X` line, in order.
    pub uses: Vec<(PartId, BTreeMap<String, f64>)>,
    /// Behavioral `require`s, checked after a transient.
    pub envelopes: Vec<Envelope>,
    /// Registry model files the body's parts need.
    pub includes: BTreeSet<String>,
}

/// The `.subckt` named `name` for `def` at `values`. Inside, nodes are the definition's own net
/// names (`GND` is node 0) and refdes are its local ones; values are written in, so the netlist
/// hash covers them. A pin nothing inside reaches (an `nc` pin) gets a 1 GΩ shunt.
pub fn body(
    def: &ComponentDef,
    reg: &Registry,
    lib: &IndexMap<PartId, ComponentDef>,
    values: &BTreeMap<String, f64>,
    name: &str,
) -> Result<Body, String> {
    let ports: Vec<String> = def.part.pins.iter().map(|p| p.name.to_ascii_lowercase()).collect();
    let shunt = |n: &str| format!("Rshunt_{n} {n} 0 {}", spice_number(crate::spice::SHUNT_OHMS));
    match &def.model {
        Model::Composite(m) => {
            let vals = composite_values(def, m, reg, lib, values)?;
            let mut out = Body {
                lines: vec![format!(".subckt {name} {}", ports.join(" "))],
                leaf_devices: 0,
                uses: Vec::new(),
                envelopes: Vec::new(),
                includes: BTreeSet::new(),
            };
            let mut net_of: BTreeMap<PinRef, String> = BTreeMap::new();
            for (net, pins) in &m.nets {
                for p in pins {
                    net_of.insert(p.clone(), if net == GND { "0".to_string() } else { net.to_ascii_lowercase() });
                }
            }
            let mut locals: Vec<&String> = m.parts.keys().collect();
            locals.sort_by_key(|r| refdes_sort_key(r));
            let mut floating = BTreeSet::new();
            for local in locals {
                let cp = &m.parts[local];
                let pdef = lookup(reg, lib, &cp.part).ok_or_else(|| format!("{local}: {} is unknown", cp.part))?;
                let mut node = |pin: &str| -> String {
                    net_of.get(&PinRef::new(local, pin)).cloned().unwrap_or_else(|| {
                        let n = format!("nc_{local}_{pin}").to_ascii_lowercase();
                        floating.insert(n.clone());
                        n
                    })
                };
                if let Some(inner) = lib.get(&cp.part) {
                    let v = vals[local.as_str()].clone();
                    let nodes: Vec<String> = pdef.pins.iter().map(|p| node(&p.name)).collect();
                    let sub = subckt_name_for(&cp.part, &inner.part, &v);
                    out.lines.push(format!("X{local} {} {sub}", nodes.join(" ")));
                    out.uses.push((cp.part.clone(), v));
                    continue;
                }
                let Some(sp) = &pdef.spice else { continue };
                if let Some(inc) = &sp.include {
                    out.includes.insert(inc.clone());
                }
                let params = vals[local.as_str()]
                    .iter()
                    .map(|(k, v)| (k.clone(), Quantity::new(*v, pdef.params[k].unit)))
                    .collect();
                let inst = PartInstance {
                    refdes: local.clone(),
                    part: cp.part.clone(),
                    params,
                    block: None,
                    origin: Origin::User,
                    pinned: None,
                };
                let at = |e: String| format!("{local}: {e}");
                if let Some(line) = &sp.line {
                    out.lines.push(
                        crate::spice::expand(line, |n| crate::spice::resolve(n, &inst, pdef, None, &mut node))
                            .map_err(at)?,
                    );
                    out.leaf_devices += 1;
                } else if let Some(line) = &sp.unit_line {
                    for unit in &pdef.units {
                        if pdef.unit_pins(unit).any(|p| net_of.contains_key(&PinRef::new(local, &p.name))) {
                            out.lines.push(
                                crate::spice::expand(line, |n| {
                                    crate::spice::resolve(n, &inst, pdef, Some(unit), &mut node)
                                })
                                .map_err(at)?,
                            );
                            out.leaf_devices += 1;
                        }
                    }
                }
            }
            for n in floating {
                out.lines.push(shunt(&n));
            }
            out.lines.push(format!(".ends {name}"));
            Ok(out)
        }
        Model::Behavioral(b) => {
            let laws = behavioral::parse(b, &def.part).map_err(|e| e.join("; "))?;
            let (lines, envelopes) = behavioral::emit(&laws, &def.part, values)?;
            let mut out = Body {
                lines: vec![format!(".subckt {name} {}", ports.join(" "))],
                leaf_devices: lines.len() as u64,
                uses: Vec::new(),
                envelopes,
                includes: BTreeSet::new(),
            };
            out.lines.extend(lines);
            for p in def.part.pins.iter().filter(|p| p.kind == PinType::Nc) {
                out.lines.push(shunt(&p.name.to_ascii_lowercase()));
            }
            out.lines.push(format!(".ends {name}"));
            Ok(out)
        }
        Model::Spice(s) => {
            let e = spice_import::compile(s, &ports, values, name)?;
            Ok(Body {
                lines: e.lines,
                leaf_devices: e.leaf_devices,
                uses: Vec::new(),
                envelopes: Vec::new(),
                includes: BTreeSet::new(),
            })
        }
    }
}

/// The definitions that use `id`, directly or through others, each after every definition it uses.
pub fn dependents_in_order(lib: &IndexMap<PartId, ComponentDef>, id: &str) -> Vec<PartId> {
    let dependents: Vec<&str> =
        lib.keys().map(String::as_str).filter(|k| *k != id && closure(lib, [*k]).contains(id)).collect();
    let mut done: BTreeSet<&str> = BTreeSet::from([id]);
    let mut out = Vec::new();
    while out.len() < dependents.len() {
        for d in &dependents {
            if !done.contains(d) && direct_uses(&lib[*d]).iter().all(|u| done.contains(u) || !dependents.contains(u)) {
                done.insert(d);
                out.push(d.to_string());
            }
        }
    }
    out
}
