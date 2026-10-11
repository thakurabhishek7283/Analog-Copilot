//! The project library (LLD §17 D1, D2, D13): the component definitions a circuit uses, embedded
//! in the circuit so a project stays a pure fold of its op log, with no library fetch on any
//! runtime. A definition is a part from the outside (pins, a refdes prefix, limits, a symbol) and
//! a model inside; an instance of it is an ordinary `PartInstance` whose `part` is the
//! definition's id (`lib:<name>`), compiled to one `.subckt` and an `X` line per instance.
//!
//! This module only reads: `lib.add`, `lib.update` and `lib.remove` in `apply` change the library,
//! after [`prepare`] has checked a definition and filled in what the core computes.

use std::collections::{BTreeMap, BTreeSet};

use indexmap::IndexMap;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::apply::{refdes_prefix, valid_net_id};
use crate::error::{ErrorCode as E, OpError};
use crate::ir::{Circuit, GND, PartId, PinRef};
use crate::registry::{Category, PartDef, PinType, Registry};
use crate::units::parse_quantity;

/// Library ids are `lib:` and a lowercase name, so they never clash with a registry part.
pub const LIB_PREFIX: &str = "lib:";
/// Nesting depth of definitions inside definitions.
pub const MAX_DEPTH: usize = 8;
/// Parts and nets inside one definition (the circuit's own limits, LLD §1, per level).
pub const MAX_DEF_PARTS: usize = 300;
pub const MAX_DEF_NETS: usize = 400;
/// Definitions in one project library.
pub const MAX_LIBRARY: usize = 64;
/// Refdes prefixes a library component may take (it compiles to an `X` line, so any of these is
/// safe in SPICE).
pub const LIBRARY_PREFIXES: [&str; 12] = ["X", "U", "XU", "XA", "IC", "TP", "SW", "RV", "K", "F", "Y", "BT"];

#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ComponentDef {
    /// The component as a part: id (`lib:<name>`), category `X`, title, refdes `prefix`, pins,
    /// limits and symbol (`auto`, a box the editor draws from the pins, or a registry symbol whose
    /// anchors match the pins). It has no `spice` line and, for now, no params.
    pub part: PartDef,
    pub version: u32,
    pub model: Model,
    pub provenance: Provenance,
}

/// What the component does inside. Behavioral laws and SPICE imports join in P5.2 (LLD §17 D3).
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Model {
    Composite(Composite),
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
    /// Values as written ("10k"); a missing param takes the part's default.
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

/// Library definitions `def` uses directly.
pub fn direct_uses(def: &ComponentDef) -> BTreeSet<&str> {
    let Model::Composite(m) = &def.model;
    m.parts.values().filter(|p| p.part.starts_with(LIB_PREFIX)).map(|p| p.part.as_str()).collect()
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

/// Primitive devices one instance of library component `id` simulates: each registry part with a
/// SPICE model counts once per unit in use, each nested component by its own count (LLD §17
/// D12's leaf-device budget). Zero for anything not in the library.
pub fn leaf_devices(reg: &Registry, lib: &IndexMap<PartId, ComponentDef>, id: &str) -> u64 {
    let Some(d) = lib.get(id) else { return 0 };
    let Model::Composite(m) = &d.model;
    let on: BTreeSet<&PinRef> = m.nets.values().flatten().collect();
    m.parts
        .iter()
        .map(|(local, p)| {
            if lib.contains_key(&p.part) {
                return leaf_devices(reg, lib, &p.part);
            }
            match reg.part(&p.part) {
                Some(pd) if pd.spice.is_some() && pd.units.is_empty() => 1,
                Some(pd) if pd.spice.is_some() => pd
                    .units
                    .iter()
                    .filter(|u| pd.unit_pins(u).any(|q| on.contains(&PinRef::new(local, &q.name))))
                    .count() as u64,
                _ => 0,
            }
        })
        .sum()
}

fn invalid(problems: Vec<String>) -> OpError {
    OpError::new(E::ComponentInvalid, problems.join("; "))
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
    if !part.params.is_empty() {
        errs.push("library components have no params yet (value rules come with P5.2)".into());
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

    let Model::Composite(m) = &def.model;
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
                    Err(e) => errs.push(format!("part {local}: {k}: {e}")),
                    Ok(q) if pdef.min.is_some_and(|lo| q.si < lo) || pdef.max.is_some_and(|hi| q.si > hi) => {
                        errs.push(format!("part {local}: {k}={} is outside the part's range", q.display))
                    }
                    Ok(_) => {}
                },
            }
        }
        defs.insert(local, pd);
    }
    let uses = direct_uses(def);
    if 1 + uses.iter().map(|u| depth(lib, u)).max().unwrap_or(0) > MAX_DEPTH {
        errs.push(format!("components nest at most {MAX_DEPTH} deep"));
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
    // Group numbers in pin order; a pin that conducts to no other pin and not to ground has none.
    let mut members: BTreeMap<usize, usize> = BTreeMap::new();
    for p in &part.pins {
        *members.entry(dsu.find(index[p.name.as_str()])).or_default() += 1;
    }
    let mut numbers: BTreeMap<usize, u16> = BTreeMap::new();
    let mut out = def.clone();
    for p in out.part.pins.iter_mut() {
        let r = dsu.find(index[p.name.as_str()]);
        p.dc_group = if r == ground_root {
            Some(0)
        } else if members[&r] > 1 {
            let next = numbers.len() as u16 + 1;
            Some(*numbers.entry(r).or_insert(next))
        } else {
            None
        };
    }
    Ok(out)
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

/// The `.subckt` name of a definition: `lib_<name>`.
pub fn subckt_name(id: &str) -> String {
    format!("lib_{}", id.strip_prefix(LIB_PREFIX).unwrap_or(id))
}
