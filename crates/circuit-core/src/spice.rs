//! SPICE compiler (LLD §8). Byte-identical output on every runtime; the netlist hash is the cache
//! key for every simulation result. Netlists are built only from registry templates and parsed
//! numbers, so neither users nor the model can inject SPICE (`.control`, `.include`, `shell`).

use std::collections::{BTreeMap, BTreeSet};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::erc::{floating_nets, implicit_supplies};
use crate::ir::*;
use crate::library;
use crate::registry::{Category, PartDef, Registry};
use crate::template::checks::{SpecCheckDef, emit as emit_checks};
use crate::units::{Unit, format_sig, spice_number};

#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
pub struct Netlist {
    /// Deterministic: parts in natural refdes order.
    pub text: String,
    /// Lowercase hex sha256 of `text`.
    pub hash: String,
    /// IR net id -> SPICE node name ("N_VOUT" -> "n_vout", "GND" -> "0").
    pub node_map: BTreeMap<NetId, String>,
    /// Registry model files the simulator must load, relative to the registry root.
    pub includes: Vec<String>,
    /// Spec checks as primitive `.meas` statements, in replay order (later cards may use earlier
    /// results).
    pub meas: Vec<MeasDef>,
    /// The spec checks of every template block, and which `meas` results each combines
    /// ([`crate::template::evaluate_checks`]).
    pub checks: Vec<SpecCheckDef>,
    /// Current into each connected pin (`"R1.1"`), as a sum of saved vectors, for the overlays
    /// and current probes. OP and transient plots only: device currents do not exist in AC.
    /// Pins inside subcircuit models (op-amps, regulators) are absent; a net with one such pin
    /// gets its current by KCL.
    pub pin_currents: BTreeMap<String, Vec<CurrentTerm>>,
    /// The analyses in the deck, in order (resolved from `CompileOpts`).
    pub analyses: Vec<Analysis>,
    /// Primitive devices the deck simulates, library components flattened (LLD §17 D12: the
    /// editor runs live below a budget and asks for a Run above it).
    #[serde(default)]
    pub leaf_devices: u64,
    /// The `require` laws of behavioral components, measured over the transient
    /// ([`evaluate_envelope`]).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub envelope: Vec<EnvelopeDef>,
}

/// A behavioral component's `require` as compiled into one netlist (LLD §17 D3, the envelope).
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
pub struct EnvelopeDef {
    /// The instance.
    pub part: RefDes,
    /// The law's number in its definition (1-based).
    pub law: usize,
    /// The law as written.
    pub text: String,
    pub unit: Unit,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lo: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hi: Option<f64>,
    /// `.meas` names of the largest and smallest value; empty when `missing` is set.
    pub meas: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub missing: Option<String>,
}

/// Whether a `require` held over the run.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
pub struct EnvelopeResult {
    pub part: RefDes,
    pub law: usize,
    pub text: String,
    pub unit: Unit,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min: Option<f64>,
    pub pass: bool,
    /// In words: the range seen, or why nothing was measured.
    pub note: String,
}

/// Combine `.meas` results into envelope results. Unmeasured counts as held: the envelope is a
/// warning about what a simulation showed, not a check that needs one.
pub fn evaluate_envelope(defs: &[EnvelopeDef], meas: &BTreeMap<String, f64>) -> Vec<EnvelopeResult> {
    defs.iter()
        .map(|d| {
            let get = |k: usize| d.meas.get(k).and_then(|m| meas.get(m)).copied();
            let (max, min) = (get(0), get(1));
            let low = d.lo.zip(min).is_some_and(|(lo, m)| m < lo - 1e-9 * lo.abs());
            let high = d.hi.zip(max).is_some_and(|(hi, m)| m > hi + 1e-9 * hi.abs());
            let note = match (&d.missing, max.zip(min)) {
                (Some(why), _) => why.clone(),
                (None, Some((hi, lo))) => {
                    format!("from {} to {}", format_sig(lo, d.unit), format_sig(hi, d.unit))
                }
                (None, None) => "not measured".to_string(),
            };
            EnvelopeResult {
                part: d.part.clone(),
                law: d.law,
                text: d.text.clone(),
                unit: d.unit,
                max,
                min,
                pass: !(low || high),
                note,
            }
        })
        .collect()
}

/// `coeff · vector`, e.g. `-1 · @r1[i]`.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
pub struct CurrentTerm {
    /// Canonical result vector name: `@r1[i]`, `i(v1)`, `@q1[ic]`.
    pub vector: String,
    pub coeff: f64,
}

#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
pub struct MeasDef {
    pub name: String,
    pub line: String,
}

#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, Default, PartialEq)]
pub struct CompileOpts {
    /// Add a 1 GΩ shunt to ground on every node without a DC path (user circuits, ERC003),
    /// so the simulation still runs and the tutor can explain the problem.
    #[serde(default)]
    pub shunt_floating: bool,
    /// Override the circuit's analyses. `None` uses `circuit.analyses`, or `.op` if that is empty.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub analyses: Option<Vec<Analysis>>,
    /// Without `analyses`: the editor's set, [`interactive_analyses`].
    #[serde(default)]
    pub interactive: bool,
}

#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq, thiserror::Error)]
#[error("{refdes:?}: {message}")]
pub struct CompileError {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refdes: Option<RefDes>,
    pub message: String,
}

fn cerr(refdes: Option<&str>, message: impl Into<String>) -> CompileError {
    CompileError { refdes: refdes.map(str::to_string), message: message.into() }
}

pub const SHUNT_OHMS: f64 = 1e9;

pub fn node_name(net: &str) -> String {
    if net == GND { "0".to_string() } else { net.to_ascii_lowercase() }
}

pub fn compile(c: &Circuit, reg: &Registry, opts: &CompileOpts) -> Result<Netlist, CompileError> {
    let pin_net = c.pin_index();
    let node_map: BTreeMap<NetId, String> = c.nets.keys().map(|k| (k.clone(), node_name(k))).collect();
    let mut includes = BTreeSet::new();
    let mut elements = Vec::new();
    let mut saves = vec!["all".to_string()];
    let mut nc_nodes = BTreeSet::new();
    let mut pin_currents = BTreeMap::new();

    let mut leaf_devices = 0;
    let mut subs = Subckts::default();
    let mut envelope_parts: Vec<(RefDes, String)> = Vec::new();
    for refdes in c.sorted_refdes() {
        let inst = &c.parts[refdes];
        let def = library::part_def(reg, c, &inst.part)
            .ok_or_else(|| cerr(Some(refdes), format!("{} is not in the registry or the library", inst.part)))?;
        if c.library.contains_key(&inst.part) {
            let values = library::instance_values(def, inst).map_err(|m| cerr(Some(refdes), m))?;
            let name = subs.ensure(c, reg, &inst.part, &values, 0).map_err(|m| cerr(Some(refdes), m))?;
            leaf_devices += subs.leaf[&name];
            envelope_parts.push((refdes.clone(), name.clone()));
            // An instance of a library component: one `X` line, pins in the definition's order.
            let mut line = format!("X{refdes}");
            for p in &def.pins {
                let n = match pin_net.get(&PinRef::new(refdes, &p.name)) {
                    Some(net) => node_name(net),
                    None => {
                        let n = format!("nc_{refdes}_{}", p.name).to_ascii_lowercase();
                        nc_nodes.insert(n.clone());
                        n
                    }
                };
                line.push(' ');
                line.push_str(&n);
            }
            line.push(' ');
            line.push_str(&name);
            elements.push(line);
            continue;
        }
        let Some(sp) = &def.spice else { continue };
        if let Some(inc) = &sp.include {
            includes.insert(inc.clone());
        }
        let before = elements.len();
        let mut node = |pin: &str| -> String {
            match pin_net.get(&PinRef::new(refdes, pin)) {
                Some(net) => node_name(net),
                None => {
                    let n = format!("nc_{refdes}_{pin}").to_ascii_lowercase();
                    nc_nodes.insert(n.clone());
                    n
                }
            }
        };
        if let Some(line) = &sp.line {
            elements.push(
                expand(line, |name| resolve(name, inst, def, None, &mut node)).map_err(|m| cerr(Some(refdes), m))?,
            );
        } else if let Some(line) = &sp.unit_line {
            for unit in &def.units {
                let used = def.unit_pins(unit).any(|p| pin_net.contains_key(&PinRef::new(refdes, &p.name)));
                if used {
                    elements.push(
                        expand(line, |name| resolve(name, inst, def, Some(unit), &mut node))
                            .map_err(|m| cerr(Some(refdes), m))?,
                    );
                }
            }
        }
        leaf_devices += (elements.len() - before) as u64;
        let r = refdes.to_ascii_lowercase();
        match def.category {
            Category::R | Category::C | Category::L => saves.push(format!("@{r}[i]")),
            Category::D => saves.push(format!("@{r}[id]")),
            Category::Q => saves.extend([format!("@{r}[ic]"), format!("@{r}[ib]")]),
            _ => {}
        }
        if let Some(line) = &sp.line {
            for (pin, terms) in terminal_currents(def, &r, line) {
                if pin_net.contains_key(&PinRef::new(refdes, &pin)) {
                    pin_currents.insert(format!("{refdes}.{pin}"), terms);
                }
            }
        }
    }

    // A rail nothing drives is an ideal supply at its declared volts (the rail flag is the source).
    for (net, volts) in implicit_supplies(c, reg) {
        let n = node_name(&net);
        elements.push(format!("Vrail_{n} {n} 0 DC {}", spice_number(volts)));
    }

    if opts.shunt_floating {
        let mut shunted: BTreeSet<String> = floating_nets(c, reg).iter().map(|n| node_name(n)).collect();
        shunted.extend(nc_nodes);
        for n in shunted.into_iter().filter(|n| n != "0") {
            elements.push(format!("Rshunt_{n} {n} 0 {}", spice_number(SHUNT_OHMS)));
        }
    }

    let analyses = match &opts.analyses {
        Some(a) => a.clone(),
        None if opts.interactive => interactive_analyses(c, reg),
        None if c.analyses.is_empty() => vec![Analysis::Op],
        None => c.analyses.clone(),
    };
    let mut analysis_lines = Vec::new();
    for a in &analyses {
        analysis_lines.push(match a {
            Analysis::Op => ".op".to_string(),
            Analysis::Dc { source, start, stop, step } => {
                if !c.parts.get(source).is_some_and(|p| reg.part(&p.part).is_some_and(|d| d.category == Category::V)) {
                    return Err(cerr(Some(source), "DC sweep source must be a voltage source in the circuit"));
                }
                format!(".dc {source} {} {} {}", spice_number(*start), spice_number(*stop), spice_number(*step))
            }
            Analysis::Ac { points_per_decade, f_start, f_stop } => {
                format!(".ac dec {points_per_decade} {} {}", spice_number(*f_start), spice_number(*f_stop))
            }
            Analysis::Tran { t_step, t_stop } => format!(".tran {} {}", spice_number(*t_step), spice_number(*t_stop)),
        });
    }

    includes.extend(subs.includes.iter().cloned());
    let includes: Vec<String> = includes.into_iter().collect();
    let mut text = String::from("* circuit-tutor netlist\n");
    for inc in &includes {
        text.push_str(&format!(".include {inc}\n"));
    }
    for lines in subs.bodies.values() {
        for line in lines {
            text.push_str(line);
            text.push('\n');
        }
    }
    for e in &elements {
        text.push_str(e);
        text.push('\n');
    }
    text.push_str(".options reltol=1e-3 gmin=1e-12\n");
    text.push_str(&format!(".save {}\n", saves.join(" ")));
    for a in &analysis_lines {
        text.push_str(a);
        text.push('\n');
    }
    let (mut meas, checks) = emit_checks(c, reg, &analyses);
    // Each behavioral instance's requires, measured over the transient.
    let has_tran = analyses.iter().any(|a| matches!(a, Analysis::Tran { .. }));
    let mut envelope = Vec::new();
    for (refdes, name) in &envelope_parts {
        for e in &subs.envelopes[name] {
            let base = format!("env_{}_{}", refdes.to_ascii_lowercase(), e.law);
            let vector = format!("v(x{}.{})", refdes.to_ascii_lowercase(), e.node);
            let mut def = EnvelopeDef {
                part: refdes.clone(),
                law: e.law,
                text: e.text.clone(),
                unit: e.unit,
                lo: e.lo,
                hi: e.hi,
                meas: Vec::new(),
                missing: None,
            };
            if has_tran {
                for (suffix, f) in [("hi", "max"), ("lo", "min")] {
                    let n = format!("{base}_{suffix}");
                    meas.push(MeasDef { line: format!(".meas tran {n} {f} {vector}"), name: n.clone() });
                    def.meas.push(n);
                }
            } else {
                def.missing = Some("needs a transient analysis".into());
            }
            envelope.push(def);
        }
    }
    for m in &meas {
        text.push_str(&m.line);
        text.push('\n');
    }
    text.push_str(".end\n");

    let hash = text_hash(&text);
    Ok(Netlist { text, hash, node_map, includes, meas, checks, pin_currents, analyses, leaf_devices, envelope })
}

/// Lowercase hex sha256 of a netlist's text.
pub(crate) fn text_hash(text: &str) -> String {
    Sha256::digest(text.as_bytes()).iter().map(|b| format!("{b:02x}")).collect()
}

/// The sub-circuits a netlist needs: one per definition and distinct param values, keyed (and
/// emitted in order) by name, nested ones included.
#[derive(Default)]
struct Subckts {
    bodies: BTreeMap<String, Vec<String>>,
    leaf: BTreeMap<String, u64>,
    envelopes: BTreeMap<String, Vec<crate::behavioral::Envelope>>,
    includes: BTreeSet<String>,
}

impl Subckts {
    /// Emit `id` at `values` (and what it uses) if not yet; returns its name.
    fn ensure(
        &mut self,
        c: &Circuit,
        reg: &Registry,
        id: &str,
        values: &BTreeMap<String, f64>,
        depth: usize,
    ) -> Result<String, String> {
        let def = c.library.get(id).ok_or_else(|| format!("{id} is not in the library"))?;
        let name = library::subckt_name_for(id, &def.part, values);
        if self.bodies.contains_key(&name) {
            return Ok(name);
        }
        if depth > library::MAX_DEPTH {
            return Err(format!("components nest more than {} deep", library::MAX_DEPTH));
        }
        let body = library::body(def, reg, &c.library, values, &name).map_err(|e| format!("{id}: {e}"))?;
        let mut leaf = body.leaf_devices;
        for (inner, v) in &body.uses {
            let n = self.ensure(c, reg, inner, v, depth + 1)?;
            leaf += self.leaf[&n];
        }
        self.includes.extend(body.includes);
        self.leaf.insert(name.clone(), leaf);
        self.envelopes.insert(name.clone(), body.envelopes);
        self.bodies.insert(name.clone(), body.lines);
        Ok(name)
    }
}

/// Pin currents of a primitive device, from SPICE's fixed terminal order (`R n+ n-`,
/// `D anode cathode`, `V n+ n-`, `Q c b e`): ngspice reports current *into* the first terminal.
fn terminal_currents(def: &PartDef, r: &str, line: &str) -> Vec<(String, Vec<CurrentTerm>)> {
    let pins: Vec<&str> = placeholders(line).filter(|n| def.pin(n).is_some()).collect();
    let t = |vector: String, coeff: f64| CurrentTerm { vector, coeff };
    let two = |v: String| match pins[..] {
        [a, b, ..] => vec![(a.to_string(), vec![t(v.clone(), 1.0)]), (b.to_string(), vec![t(v, -1.0)])],
        _ => Vec::new(),
    };
    match def.category {
        Category::R | Category::C | Category::L => two(format!("@{r}[i]")),
        Category::D => two(format!("@{r}[id]")),
        Category::V => two(format!("i({r})")),
        Category::Q => match pins[..] {
            [c, b, e, ..] => {
                let (ic, ib) = (format!("@{r}[ic]"), format!("@{r}[ib]"));
                vec![
                    (c.to_string(), vec![t(ic.clone(), 1.0)]),
                    (b.to_string(), vec![t(ib.clone(), 1.0)]),
                    (e.to_string(), vec![t(ic, -1.0), t(ib, -1.0)]),
                ]
            }
            _ => Vec::new(),
        },
        _ => Vec::new(),
    }
}

/// `{name}` placeholders of a template line, in order.
fn placeholders(tpl: &str) -> impl Iterator<Item = &str> {
    tpl.split('{').skip(1).filter_map(|s| s.split_once('}').map(|(name, _)| name))
}

/// Default short transient for the editor (LLD §8): five periods of the slowest periodic source,
/// or 10 ms without one, in about 1,000 steps.
pub const DEFAULT_TRAN_PERIODS: f64 = 5.0;
pub const DEFAULT_TRAN_STOP: f64 = 10e-3;
pub const DEFAULT_TRAN_STEPS: f64 = 1000.0;

/// What the editor simulates after every edit (LLD §8): the operating point, the circuit's own
/// transient or else a short default one, then the circuit's other analyses (AC, DC sweeps).
/// AC and long transients therefore run only once someone asked for them with `analysis.set`.
pub fn interactive_analyses(c: &Circuit, reg: &Registry) -> Vec<Analysis> {
    let mut out = vec![Analysis::Op];
    match c.analyses.iter().find(|a| matches!(a, Analysis::Tran { .. })) {
        Some(tran) => out.push(tran.clone()),
        None => {
            let slowest = c
                .parts
                .values()
                .filter_map(|p| reg.part(&p.part).filter(|d| d.category == Category::V).map(|_| p))
                .flat_map(|p| p.params.values())
                .filter(|q| q.unit == Unit::Hertz && q.si > 0.0)
                .map(|q| q.si)
                .reduce(f64::min);
            let t_stop = slowest.map_or(DEFAULT_TRAN_STOP, |f| (DEFAULT_TRAN_PERIODS / f).clamp(1e-6, 1.0));
            out.push(Analysis::Tran { t_step: t_stop / DEFAULT_TRAN_STEPS, t_stop });
        }
    }
    out.extend(c.analyses.iter().filter(|a| !matches!(a, Analysis::Op | Analysis::Tran { .. })).cloned());
    out
}

pub(crate) fn resolve(
    name: &str,
    inst: &PartInstance,
    def: &PartDef,
    unit: Option<&str>,
    node: &mut impl FnMut(&str) -> String,
) -> Option<String> {
    match (name, unit) {
        ("refdes", _) => return Some(inst.refdes.clone()),
        ("unit", Some(u)) => return Some(u.to_string()),
        _ => {}
    }
    let pin = match unit {
        Some(u) => def.resolve_unit_pin(u, name),
        None => def.pin(name),
    };
    if let Some(p) = pin {
        return Some(node(&p.name));
    }
    inst.params.get(name).map(|q| q.to_spice())
}

pub(crate) fn expand(tpl: &str, mut resolve: impl FnMut(&str) -> Option<String>) -> Result<String, String> {
    let mut out = String::with_capacity(tpl.len() + 16);
    let mut rest = tpl;
    while let Some(open) = rest.find('{') {
        out.push_str(&rest[..open]);
        let close = rest[open..].find('}').ok_or_else(|| format!("unbalanced brace in \"{tpl}\""))?;
        let name = &rest[open + 1..open + close];
        out.push_str(&resolve(name).ok_or_else(|| format!("cannot resolve {{{name}}} in \"{tpl}\""))?);
        rest = &rest[open + close + 1..];
    }
    out.push_str(rest);
    Ok(out)
}
