//! Grounded tutor (LLD §9), shared by the API and the browser. [`context`] renders the slice of
//! the circuit a question is about, with the learner's own simulation values, as compact text for
//! the model. [`read_answer`] finds the references an answer cites (`[R3]`, `[net:N_A]`,
//! `[block:b2]`) and checks each against the circuit, and validates the answer's `try` block as
//! ops on a scratch copy, so a suggestion the learner is shown always applies. [`changes`]
//! renders what an edit changed ("What changed?"): the edit and the simulation before and after.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use indexmap::IndexSet;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::apply::apply_ops;
use crate::erc::{ErcContext, erc};
use crate::error::{ErrorCode, OpError};
use crate::ir::{BlockId, Circuit, NetId, NetKind, PinRef, RefDes, refdes_sort_key};
use crate::ops::{Author, Op};
use crate::registry::{Hazard, Registry};
use crate::template::CheckResult;
use crate::units::{Unit, format_sig};

mod changes;
pub use changes::{ChangeContext, MAX_CHANGED_NETS, changes};

/// At most this many parts in a context (LLD §9).
pub const MAX_CONTEXT_PARTS: usize = 15;
/// A context's budget in estimated tokens (characters / 4, as the prompt budgets in LLD §13).
pub const CONTEXT_TOKENS: usize = 2000;

/// What the learner has selected in the editor.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Selection {
    Part { refdes: RefDes },
    Net { id: NetId },
    Block { id: BlockId },
}

/// Explain, or ask one guiding question first (LLD §9, rule 4).
#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TutorMode {
    #[default]
    Explain,
    Socratic,
}

/// The learner's simulation of the circuit the question is about. The server never simulated an
/// edited circuit, so these come from the browser (LLD §5); the circuit itself does not.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, Default, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct SimValues {
    /// The run's status (`ok`, `no_convergence`, `singular_matrix`, `timeout`, `error`); absent
    /// when nothing was simulated.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    /// Operating point: net -> volts.
    #[serde(default)]
    pub op_v: BTreeMap<NetId, f64>,
    /// Operating point: pin (`R3.1`) -> amps flowing into the pin.
    #[serde(default)]
    pub op_i: BTreeMap<String, f64>,
    /// AC response at the scope's frequency.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ac: Option<AcValues>,
    /// The transient's range on each net, after start-up.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tran: Option<TranValues>,
    /// Spec checks as the browser measured them.
    #[serde(default)]
    pub checks: Vec<CheckResult>,
}

#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct AcValues {
    pub hz: f64,
    /// Net -> magnitude (volts).
    pub v: BTreeMap<NetId, f64>,
    /// Net -> phase (degrees).
    #[serde(default)]
    pub deg: BTreeMap<NetId, f64>,
}

#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct TranValues {
    pub t_stop: f64,
    /// Net -> lowest and highest voltage over the run's second half (start-up swings left out).
    pub v: BTreeMap<NetId, Span>,
}

#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, Debug, PartialEq)]
pub struct Span {
    pub min: f64,
    pub max: f64,
}

/// The slice of a circuit a question is about, as the model sees it.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
pub struct TutorContext {
    /// Compact text for the prompt (LLD §9).
    pub text: String,
    /// The parts, nets and blocks the text describes.
    pub parts: Vec<RefDes>,
    pub nets: Vec<NetId>,
    pub blocks: Vec<BlockId>,
    /// Parts left out for the part limit or the token budget, nearest the selection first.
    pub dropped: Vec<RefDes>,
    /// Parts anywhere in the circuit flagged `hazard: mains`: the answer gets a safety note.
    pub hazards: Vec<RefDes>,
    /// Estimated tokens of `text`.
    pub tokens: u32,
}

/// Render the slice of `c` a question is about (LLD §9, context builder rules):
/// - a part selected: the part, the rest of its block, and every part one hop away over signal nets;
/// - a net selected: every part on it and the net's values;
/// - a block selected: the block's parts, without neighbours;
/// - parts the question names (`R3`, `r3`) are added; with nothing selected they are the seeds;
///   with neither, the whole circuit in refdes order.
///
/// Supply and ground rails are never walked (every part would be one hop from every other); they
/// get one line each. Beyond [`MAX_CONTEXT_PARTS`] or [`CONTEXT_TOKENS`], the parts farthest from
/// the selection are dropped first, never the selection itself.
pub fn context(
    c: &Circuit,
    reg: &Registry,
    question: &str,
    selection: Option<&Selection>,
    sim: &SimValues,
) -> Result<TutorContext, OpError> {
    let (named_parts, named_nets, named_blocks) = mentions(c, question);
    let mut seeds: Vec<RefDes> = Vec::new();
    let mut walk = true;
    let mut first_nets: Vec<NetId> = Vec::new();
    let mut first_blocks: Vec<BlockId> = Vec::new();
    match selection {
        Some(Selection::Part { refdes }) => {
            if !c.parts.contains_key(refdes) {
                return Err(OpError::new(ErrorCode::PartNotFound, format!("no part {refdes} at rev {}", c.rev)));
            }
            seeds.push(refdes.clone());
        }
        Some(Selection::Net { id }) => {
            let net = c.nets.get(id).ok_or_else(|| OpError::new(ErrorCode::NetNotFound, format!("no net {id}")))?;
            first_nets.push(id.clone());
            if net.kind == NetKind::Signal {
                seeds.extend(sorted(net.pins.iter().map(|p| p.refdes.clone())));
            }
            walk = false;
        }
        Some(Selection::Block { id }) => {
            if !c.blocks.contains_key(id) {
                return Err(OpError::new(ErrorCode::BlockNotFound, format!("no block {id}")));
            }
            first_blocks.push(id.clone());
            seeds.extend(sorted(c.parts.values().filter(|p| p.block.as_ref() == Some(id)).map(|p| p.refdes.clone())));
            walk = false;
        }
        None => {}
    }

    // Parts in priority order: the selection, the parts the question names, the rest of their
    // blocks, then neighbours (or, with nothing to start from, every part).
    let mut ranked: IndexSet<RefDes> = seeds.iter().cloned().collect();
    let protected = ranked.len().max(1);
    ranked.extend(named_parts.iter().cloned());
    if selection.is_none() && !named_parts.is_empty() {
        seeds = named_parts.clone();
    }
    if walk {
        // A part brings the rest of its block: what the block does (its cutoff, its gain) depends
        // on every part in it, not only on the parts one hop away.
        let blocks: IndexSet<&BlockId> = seeds.iter().filter_map(|r| c.parts.get(r)?.block.as_ref()).collect();
        for b in blocks {
            ranked.extend(sorted(c.parts.values().filter(|p| p.block.as_ref() == Some(b)).map(|p| p.refdes.clone())));
        }
    }
    if seeds.is_empty() {
        ranked.extend(c.sorted_refdes().into_iter().cloned());
    } else if walk || selection.is_none() {
        let dist = distances(c, &seeds);
        let mut near: Vec<(&RefDes, usize)> = dist.iter().filter(|(_, d)| **d == 1).map(|(r, d)| (*r, *d)).collect();
        near.sort_by_key(|(r, d)| (*d, refdes_sort_key(r)));
        ranked.extend(near.into_iter().map(|(r, _)| r.clone()));
    }
    let mut parts: Vec<RefDes> = ranked.into_iter().collect();
    let mut dropped: Vec<RefDes> =
        if parts.len() > MAX_CONTEXT_PARTS { parts.split_off(MAX_CONTEXT_PARTS) } else { vec![] };

    let hazards = mains_parts(c, reg);
    let issues = erc(c, reg, ErcContext::UserEdit, None);

    loop {
        let slice = Slice::new(c, &parts, &first_nets, &named_nets, &first_blocks, &named_blocks);
        let text = render(c, reg, &slice, selection, sim, &issues, &hazards);
        let tokens = text.chars().count().div_ceil(4);
        if tokens <= CONTEXT_TOKENS || parts.len() <= protected {
            let Slice { nets, blocks, .. } = slice;
            return Ok(TutorContext { text, parts, nets, blocks, dropped, hazards, tokens: tokens as u32 });
        }
        dropped.insert(0, parts.pop().expect("more parts than protected"));
    }
}

/// Parts anywhere in `c` flagged `hazard: mains`, in refdes order.
fn mains_parts(c: &Circuit, reg: &Registry) -> Vec<RefDes> {
    c.sorted_refdes()
        .into_iter()
        .filter(|r| reg.part(&c.parts[*r].part).is_some_and(|d| d.hazard == Some(Hazard::Mains)))
        .cloned()
        .collect()
}

fn sorted(it: impl Iterator<Item = RefDes>) -> Vec<RefDes> {
    let mut v: Vec<RefDes> = it.collect::<BTreeSet<_>>().into_iter().collect();
    v.sort_by_key(|r| refdes_sort_key(r));
    v
}

/// Parts (any case), nets and blocks a question names, in the order it names them.
fn mentions(c: &Circuit, question: &str) -> (Vec<RefDes>, Vec<NetId>, Vec<BlockId>) {
    let upper: BTreeMap<String, &RefDes> = c.parts.keys().map(|r| (r.to_ascii_uppercase(), r)).collect();
    let (mut parts, mut nets, mut blocks) = (IndexSet::new(), IndexSet::new(), IndexSet::new());
    for token in question.split(|ch: char| !(ch.is_ascii_alphanumeric() || ch == '_')).filter(|t| !t.is_empty()) {
        if let Some(r) = upper.get(&token.to_ascii_uppercase()) {
            parts.insert((*r).clone());
        } else if c.nets.contains_key(token) {
            nets.insert(token.to_string());
        } else if c.blocks.contains_key(token) {
            blocks.insert(token.to_string());
        }
    }
    (parts.into_iter().collect(), nets.into_iter().collect(), blocks.into_iter().collect())
}

/// Hops from `seeds` to every part reachable over signal nets.
fn distances<'a>(c: &'a Circuit, seeds: &[RefDes]) -> BTreeMap<&'a RefDes, usize> {
    let mut nets_of: BTreeMap<&str, Vec<&NetId>> = BTreeMap::new();
    for net in c.nets.values().filter(|n| n.kind == NetKind::Signal) {
        for p in &net.pins {
            nets_of.entry(p.refdes.as_str()).or_default().push(&net.id);
        }
    }
    let mut dist: BTreeMap<&RefDes, usize> = BTreeMap::new();
    let mut queue = VecDeque::new();
    for s in seeds {
        if let Some((r, _)) = c.parts.get_key_value(s) {
            dist.insert(r, 0);
            queue.push_back(r);
        }
    }
    while let Some(r) = queue.pop_front() {
        let d = dist[r];
        for net in nets_of.get(r.as_str()).into_iter().flatten() {
            for p in &c.nets[*net].pins {
                if let Some((other, _)) = c.parts.get_key_value(&p.refdes) {
                    if !dist.contains_key(other) {
                        dist.insert(other, d + 1);
                        queue.push_back(other);
                    }
                }
            }
        }
    }
    dist
}

/// The nets and blocks a set of parts touches, plus those selected or named.
struct Slice<'a> {
    parts: &'a [RefDes],
    nets: Vec<NetId>,
    blocks: Vec<BlockId>,
}

impl<'a> Slice<'a> {
    fn new(
        c: &Circuit,
        parts: &'a [RefDes],
        first_nets: &[NetId],
        named_nets: &[NetId],
        first_blocks: &[BlockId],
        named_blocks: &[BlockId],
    ) -> Slice<'a> {
        let pin_net = c.pin_index();
        let mut nets: IndexSet<NetId> = first_nets.iter().cloned().collect();
        let mut blocks: IndexSet<BlockId> = first_blocks.iter().cloned().collect();
        for r in parts {
            let inst = &c.parts[r];
            if let Some(b) = &inst.block {
                blocks.insert(b.clone());
            }
            let mut mine: Vec<&NetId> = pin_net.iter().filter(|(p, _)| &p.refdes == r).map(|(_, n)| *n).collect();
            mine.dedup();
            nets.extend(mine.into_iter().cloned());
        }
        nets.extend(named_nets.iter().cloned());
        blocks.extend(named_blocks.iter().cloned());
        let mut blocks: Vec<BlockId> = blocks.into_iter().collect();
        blocks.sort_by_key(|b| c.blocks.get_index_of(b));
        Slice { parts, nets: nets.into_iter().collect(), blocks }
    }
}

fn v(x: f64) -> String {
    format_sig(x, Unit::Volt)
}

fn render(
    c: &Circuit,
    reg: &Registry,
    s: &Slice,
    selection: Option<&Selection>,
    sim: &SimValues,
    issues: &[crate::erc::ErcIssue],
    hazards: &[RefDes],
) -> String {
    let mut out = String::new();
    let line = |out: &mut String, text: String| {
        out.push_str(&text);
        out.push('\n');
    };
    line(
        &mut out,
        format!("SIM: {}", sim.status.as_deref().unwrap_or("not run (no simulation results for this circuit)")),
    );
    match selection {
        Some(Selection::Part { refdes }) => line(&mut out, format!("SELECTED: part {refdes}")),
        Some(Selection::Net { id }) => line(&mut out, format!("SELECTED: net {id}")),
        Some(Selection::Block { id }) => line(&mut out, format!("SELECTED: block {id}")),
        None => line(&mut out, "SELECTED: nothing".into()),
    }

    for id in &s.blocks {
        let b = &c.blocks[id];
        let t = b.template.as_ref().and_then(|t| reg.templates.get(t));
        let what = b.template.as_deref().map(|t| format!(" template {t},")).unwrap_or_default();
        line(&mut out, format!("BLOCK {id} \"{}\"{what} {}, {}", b.title, word(b.role), word(b.status)));
        let ports: Vec<String> = b.ports.iter().map(|p| format!("{}:{}", p.name, p.net)).collect();
        if !ports.is_empty() {
            line(&mut out, format!("  ports: {}", ports.join(" ")));
        }
        let measured: Vec<&CheckResult> = sim.checks.iter().filter(|r| &r.block == id).collect();
        let mut specs = Vec::new();
        for (name, spec) in &b.spec {
            let unit =
                t.and_then(|t| t.checks.iter().find(|ch| &ch.name == name)).map_or(Unit::Unitless, |ch| ch.kind.unit());
            let mut s = format!("{name} {} ±{}%", format_sig(spec.target, unit), spec.tol_pct);
            if let Some(r) = measured.iter().find(|r| &r.name == name) {
                match &r.measured_display {
                    Some(m) => s.push_str(&format!(" measured {m} {}", if r.pass { "pass" } else { "FAIL" })),
                    None => s.push_str(&format!(" not measured ({})", r.note.as_deref().unwrap_or("no result"))),
                }
            }
            specs.push(s);
        }
        if !specs.is_empty() {
            line(&mut out, format!("  spec: {}", specs.join("; ")));
        }
        if let Some(teach) = t.and_then(|t| t.teach.as_deref()) {
            line(&mut out, format!("  teaches: {teach}"));
        }
    }

    if !s.parts.is_empty() {
        line(&mut out, "PARTS:".into());
    }
    let pin_net = c.pin_index();
    for r in s.parts {
        let inst = &c.parts[r];
        let def = reg.part(&inst.part);
        let mut row = format!("  {r} {}", inst.part);
        if let Some(d) = def {
            row.push_str(&format!(" \"{}\"", d.title));
        }
        if let Some(b) = &inst.block {
            row.push_str(&format!(" [{b}]"));
        }
        for (k, q) in &inst.params {
            row.push_str(&format!(" {k}={}", q.display));
        }
        let pins: Vec<&str> = def.map(|d| d.pins.iter().map(|p| p.name.as_str()).collect()).unwrap_or_default();
        for p in &pins {
            let net = pin_net.get(&PinRef::new(r, *p)).map(|n| n.as_str()).unwrap_or("-");
            row.push_str(&format!(" {p}:{net}"));
        }
        // Currents at the operating point: a two-pin part's one current, else each pin's.
        let currents: Vec<(&str, f64)> =
            pins.iter().filter_map(|p| sim.op_i.get(&format!("{r}.{p}")).map(|i| (*p, *i))).collect();
        if pins.len() == 2 && !currents.is_empty() {
            let (p, i) = currents[0];
            row.push_str(&format!(" | I={} into {p}", format_sig(i, Unit::Ampere)));
        } else if !currents.is_empty() {
            let each: Vec<String> =
                currents.iter().map(|(p, i)| format!("{p}={}", format_sig(*i, Unit::Ampere))).collect();
            row.push_str(&format!(" | I into {}", each.join(" ")));
        }
        line(&mut out, row);
        if matches!(selection, Some(Selection::Part { refdes }) if refdes == r) {
            if let Some(teach) = def.and_then(|d| d.teach.as_deref()) {
                line(&mut out, format!("    teaches: {teach}"));
            }
        }
    }

    let (signal, rails): (Vec<&NetId>, Vec<&NetId>) =
        s.nets.iter().filter(|n| c.nets.contains_key(*n)).partition(|n| c.nets[*n].kind == NetKind::Signal);
    if !signal.is_empty() {
        line(&mut out, "NETS:".into());
    }
    for id in signal {
        let pins: Vec<String> = c.nets[id].pins.iter().map(|p| p.to_string()).collect();
        line(&mut out, format!("  {id}: {}{}", pins.join(" "), values(id, sim)));
    }
    if !rails.is_empty() {
        let each: Vec<String> = rails
            .iter()
            .map(|id| {
                let kind = match c.nets[*id].kind {
                    NetKind::Power { volts } => format!("power {}", v(volts)),
                    _ => "ground".to_string(),
                };
                format!("{id} {kind}{}", values(id, sim))
            })
            .collect();
        line(&mut out, format!("RAILS: {}", each.join("; ")));
    }

    let in_slice = |i: &&crate::erc::ErcIssue| {
        i.parts.iter().any(|p| s.parts.contains(p))
            || i.nets.iter().any(|n| s.nets.contains(n))
            || i.pins.iter().any(|p| s.parts.contains(&p.refdes))
            || i.block.as_ref().is_some_and(|b| s.blocks.contains(b))
    };
    let mine: Vec<&crate::erc::ErcIssue> = issues.iter().filter(in_slice).collect();
    if mine.is_empty() {
        line(&mut out, "ERC: none".into());
    } else {
        line(&mut out, "ERC:".into());
        for i in mine {
            line(&mut out, format!("  {} {}: {}", i.rule, word(i.severity), i.message));
        }
    }
    if !hazards.is_empty() {
        line(&mut out, format!("SAFETY: mains-powered parts in this circuit: {}", hazards.join(", ")));
    }
    out
}

/// A net's simulated values: ` | op 4.5V | ac 980mV -12° @100Hz | tran 500mV..8.5V`.
fn values(net: &str, sim: &SimValues) -> String {
    let mut s = String::new();
    if let Some(x) = sim.op_v.get(net) {
        s.push_str(&format!(" | op {}", v(*x)));
    }
    if let Some(ac) = &sim.ac {
        if let Some(m) = ac.v.get(net) {
            let deg = ac.deg.get(net).map(|d| format!(" {}°", format_sig(*d, Unit::Unitless))).unwrap_or_default();
            s.push_str(&format!(" | ac {}{deg} @{}", v(*m), format_sig(ac.hz, Unit::Hertz)));
        }
    }
    if let Some(span) = sim.tran.as_ref().and_then(|t| t.v.get(net)) {
        s.push_str(&format!(" | tran {}..{}", v(span.min), v(span.max)));
    }
    s
}

/// An enum's wire name (`filter`, `committed`, `warning`).
fn word<T: Serialize>(t: T) -> String {
    serde_json::to_value(t).ok().and_then(|v| v.as_str().map(str::to_string)).unwrap_or_default()
}

// ---------------------------------------------------------------- answers

#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RefKind {
    Part,
    Net,
    Block,
}

/// One reference an answer cites, located in [`Answer::body`].
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
pub struct Ref {
    pub kind: RefKind,
    pub id: String,
    /// It names something in the circuit; an invalid one is shown as plain text (LLD §9).
    pub valid: bool,
    /// The bracketed token's span in `body`, in UTF-16 code units (JavaScript string indices).
    pub start: u32,
    pub end: u32,
}

/// An experiment the tutor suggests (LLD §9, rule 5): ops the learner can apply, and what the
/// tutor predicts will happen.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
pub struct TrySuggestion {
    pub ops: Vec<Op>,
    pub predict: String,
    /// Why it cannot be applied (a malformed block, an op a learner cannot make, or what `apply`
    /// rejected). Empty when the ops apply to the circuit as one undoable step.
    pub problems: Vec<OpError>,
}

/// A tutor answer, read against the circuit it is about.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
pub struct Answer {
    /// The answer without its `try` block.
    pub body: String,
    pub refs: Vec<Ref>,
    pub refs_valid: u32,
    pub refs_invalid: u32,
    #[serde(rename = "try", default, skip_serializing_if = "Option::is_none")]
    pub try_: Option<TrySuggestion>,
}

/// Read an answer (complete, or streamed so far) against `c`.
pub fn read_answer(c: &Circuit, reg: &Registry, text: &str) -> Answer {
    let (body, block) = split_try(text);
    let refs = scan_refs(c, &body);
    let valid = refs.iter().filter(|r| r.valid).count() as u32;
    Answer {
        refs_valid: valid,
        refs_invalid: refs.len() as u32 - valid,
        refs,
        body,
        try_: block.map(|b| read_try(c, reg, &b)),
    }
}

/// The answer without its fenced ```` ```try ```` block, and the block's contents. An unclosed
/// block (a stream still arriving) is cut from the body but not read.
fn split_try(text: &str) -> (String, Option<String>) {
    let Some(open) = text.find("```try") else { return (text.trim_end().to_string(), None) };
    let after = &text[open + "```try".len()..];
    let Some(nl) = after.find('\n') else { return (text[..open].trim_end().to_string(), None) };
    let inner = &after[nl + 1..];
    let Some(close) = inner.find("```") else { return (text[..open].trim_end().to_string(), None) };
    let rest = inner[close + 3..].trim();
    let mut body = text[..open].trim_end().to_string();
    if !rest.is_empty() {
        body.push_str("\n\n");
        body.push_str(rest);
    }
    (body, Some(inner[..close].to_string()))
}

/// The ops a learner can make from a suggestion: changing, swapping, adding, removing and wiring
/// parts, and the analyses. Blocks, hints, pinning and narration stay with the editor and jobs.
fn learner_op(op: &Op) -> bool {
    matches!(
        op,
        Op::PartSetParam(_)
            | Op::PartSwap(_)
            | Op::PartAdd(_)
            | Op::PartRemove(_)
            | Op::NetConnect(_)
            | Op::NetDisconnect(_)
            | Op::AnalysisSet(_)
    )
}

fn read_try(c: &Circuit, reg: &Registry, block: &str) -> TrySuggestion {
    let schema = |msg: String| OpError::new(ErrorCode::SchemaError, msg);
    let mut t = TrySuggestion { ops: Vec::new(), predict: String::new(), problems: Vec::new() };
    let v: serde_json::Value = match serde_json::from_str(block.trim()) {
        Ok(v) => v,
        Err(e) => {
            t.problems.push(schema(format!("try block: {e}")));
            return t;
        }
    };
    t.predict = v.get("predict").and_then(|p| p.as_str()).unwrap_or_default().to_string();
    let Some(ops) = v.get("ops").and_then(|o| o.as_array()) else {
        t.problems.push(schema("try block: no \"ops\" list".into()));
        return t;
    };
    for (i, raw) in ops.iter().enumerate() {
        match serde_json::from_value::<Op>(raw.clone()) {
            Ok(op) if learner_op(&op) => t.ops.push(op),
            Ok(_) => {
                let mut e =
                    OpError::new(ErrorCode::Forbidden, "a suggestion can only change, add, remove or wire parts");
                e.op_index = Some(i);
                t.problems.push(e);
            }
            Err(err) => {
                let mut e = schema(format!("try op {i}: {err}"));
                e.op_index = Some(i);
                t.problems.push(e);
            }
        }
    }
    if t.problems.is_empty() {
        if t.ops.is_empty() {
            t.problems.push(schema("try block: no ops".into()));
        } else if let Err(e) = apply_ops(c, reg, &t.ops, Author::User, None) {
            t.problems.push(e);
        }
    }
    t
}

/// Bracketed references: `[R3]` (letters then digits), `[net:N_A]`, `[block:b2]`. A Markdown
/// link (`[text](url)`) and anything else in brackets is not a reference.
fn scan_refs(c: &Circuit, body: &str) -> Vec<Ref> {
    let chars: Vec<char> = body.chars().collect();
    let mut at16 = Vec::with_capacity(chars.len() + 1);
    let mut n = 0u32;
    for ch in &chars {
        at16.push(n);
        n += ch.len_utf16() as u32;
    }
    at16.push(n);
    let mut refs = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '[' {
            let close = chars[i + 1..].iter().take(72).position(|&ch| ch == ']' || ch == '[' || ch == '\n');
            if let Some(k) = close.filter(|k| chars[i + 1 + k] == ']') {
                let end = i + 1 + k;
                let inner: String = chars[i + 1..end].iter().collect();
                if chars.get(end + 1) != Some(&'(') {
                    if let Some((kind, id)) = parse_ref(&inner) {
                        let valid = match kind {
                            RefKind::Part => c.parts.contains_key(&id),
                            RefKind::Net => c.nets.contains_key(&id),
                            RefKind::Block => c.blocks.contains_key(&id),
                        };
                        refs.push(Ref { kind, id, valid, start: at16[i], end: at16[end + 1] });
                        i = end + 1;
                        continue;
                    }
                }
            }
        }
        i += 1;
    }
    refs
}

fn parse_ref(inner: &str) -> Option<(RefKind, String)> {
    let ident = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_');
    if let Some(id) = inner.strip_prefix("net:") {
        return ident(id).then(|| (RefKind::Net, id.to_string()));
    }
    if let Some(id) = inner.strip_prefix("block:") {
        return ident(id).then(|| (RefKind::Block, id.to_string()));
    }
    let digits = inner.find(|ch: char| ch.is_ascii_digit())?;
    let refdes = digits > 0
        && inner[..digits].bytes().all(|b| b.is_ascii_alphabetic())
        && inner[digits..].bytes().all(|b| b.is_ascii_digit());
    refdes.then(|| (RefKind::Part, inner.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_references_but_not_links_or_footnotes() {
        let c = Circuit::new("t");
        let refs = scan_refs(&c, "Raise [R3] (see [net:N_A], [block:b2]) — [1], [docs](http://x), [R3.1], [µ] [U12]");
        let got: Vec<(RefKind, &str)> = refs.iter().map(|r| (r.kind, r.id.as_str())).collect();
        assert_eq!(got, [(RefKind::Part, "R3"), (RefKind::Net, "N_A"), (RefKind::Block, "b2"), (RefKind::Part, "U12")]);
        assert!(refs.iter().all(|r| !r.valid), "an empty circuit has none of them");
        // UTF-16 spans: "—" and "µ" are one code unit each.
        let text: Vec<u16> = "Raise [R3] (see [net:N_A], [block:b2]) — [1], [docs](http://x), [R3.1], [µ] [U12]"
            .encode_utf16()
            .collect();
        let last = &refs[3];
        assert_eq!(String::from_utf16(&text[last.start as usize..last.end as usize]).unwrap(), "[U12]");
    }

    #[test]
    fn splits_the_try_block_from_the_body() {
        let (body, block) = split_try("Halve it.\n\n```try\n{\"ops\":[]}\n```\n");
        assert_eq!((body.as_str(), block.as_deref()), ("Halve it.", Some("{\"ops\":[]}\n")));
        let (body, block) = split_try("Halve it.\n```try\n{\"ops\":[");
        assert_eq!((body.as_str(), block), ("Halve it.", None), "a block still streaming is hidden");
        let (body, _) = split_try("A\n```try\n{}\n```\nThen look at the scope.");
        assert_eq!(body, "A\n\nThen look at the scope.");
    }
}
