//! "What changed?" (LLD §9): what an edit did, for the model to explain. The model gets the edit
//! (what differs between the circuits at the two revs, in words) and the learner's simulation
//! before and after it: spec checks, net voltages, the edited parts' currents and ERC findings.
//! Never the circuits themselves.

use std::collections::{BTreeMap, BTreeSet};

use indexmap::IndexSet;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::{CONTEXT_TOKENS, SimValues, mains_parts, v, word};
use crate::erc::{ErcContext, ErcIssue, erc};
use crate::ir::{Analysis, BlockId, Circuit, NetId, PinRef, RefDes, refdes_sort_key};
use crate::registry::Registry;
use crate::template::CheckResult;
use crate::units::{Unit, format_sig};

/// A value counts as moved when it changed by more than 1% of the larger of the two ...
const REL: f64 = 0.01;
/// ... and by more than 1 mV, 1 nA or 1° (phase).
const ABS_V: f64 = 1e-3;
const ABS_I: f64 = 1e-9;
const ABS_DEG: f64 = 1.0;
/// At most this many lines describe the edit (a whole block added or removed is one line).
const MAX_EDIT_LINES: usize = 20;
/// At most this many nets are listed, largest change first.
pub const MAX_CHANGED_NETS: usize = 12;
const MAX_CURRENT_PARTS: usize = 8;
const MAX_ERC_LINES: usize = 8;

/// What an edit changed, as the model sees it.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
pub struct ChangeContext {
    /// Compact text for the prompt.
    pub text: String,
    /// Parts the edit added, removed or changed, in refdes order.
    pub parts: Vec<RefDes>,
    /// Blocks the edit touched.
    pub blocks: Vec<BlockId>,
    /// Nets whose values the text lists, largest change first.
    pub nets: Vec<NetId>,
    /// Spec checks whose result moved (or that appeared or went).
    pub checks_moved: u32,
    /// Parts flagged `hazard: mains` in the circuit after the edit: the answer gets a safety note.
    pub hazards: Vec<RefDes>,
    /// Estimated tokens of `text`.
    pub tokens: u32,
}

/// Describe what changed from `before` to `after` (LLD §9, "What changed?"):
/// - the edit: parts added, removed, swapped, re-valued or re-wired, whole blocks, analyses;
/// - the spec checks before → after (moved ones first, the rest on one line);
/// - every net whose voltage moved by more than 1% and 1 mV (operating point, AC magnitude and
///   phase at the scope's frequency, transient range), largest change first, at most
///   [`MAX_CHANGED_NETS`];
/// - the edited parts' currents at the operating point, and ERC findings that appeared or went.
///
/// Over [`CONTEXT_TOKENS`], the nets that moved least are dropped first.
pub fn changes(
    before: &Circuit,
    after: &Circuit,
    reg: &Registry,
    sim_before: &SimValues,
    sim_after: &SimValues,
) -> ChangeContext {
    let edit = edit(before, after, reg);
    let (check_lines, checks_moved) = checks(&sim_before.checks, &sim_after.checks);
    let (mut rows, still) = voltages(before, after, sim_before, sim_after);
    let mut left_out = rows.len().saturating_sub(MAX_CHANGED_NETS);
    rows.truncate(MAX_CHANGED_NETS);
    let currents = currents(&edit.parts, before, after, reg, sim_before, sim_after);
    let erc = erc_lines(before, after, reg);
    let hazards = mains_parts(after, reg);
    let simulated = |s: &SimValues| !s.op_v.is_empty() || s.ac.is_some() || s.tran.is_some();
    let compared = simulated(sim_before) && simulated(sim_after);

    loop {
        let mut out = String::new();
        let mut line = |text: String| {
            out.push_str(&text);
            out.push('\n');
        };
        line(format!("EDIT (rev {} → {}):", before.rev, after.rev));
        for l in edit.lines.iter().take(MAX_EDIT_LINES) {
            line(format!("  {l}"));
        }
        if edit.lines.len() > MAX_EDIT_LINES {
            line(format!("  and {} more changes", edit.lines.len() - MAX_EDIT_LINES));
        }
        for id in &edit.blocks {
            let Some(b) = after.blocks.get(id).or_else(|| before.blocks.get(id)) else { continue };
            let what = b.template.as_deref().map(|t| format!(" template {t},")).unwrap_or_default();
            line(format!("BLOCK {id} \"{}\"{what} {}", b.title, word(b.role)));
            let teach = b.template.as_ref().and_then(|t| reg.templates.get(t)).and_then(|t| t.teach.as_deref());
            if let Some(teach) = teach {
                line(format!("  teaches: {teach}"));
            }
        }
        let status = |s: &SimValues| s.status.clone().unwrap_or_else(|| "not run".into());
        line(format!("SIM: {} → {}", status(sim_before), status(sim_after)));
        if check_lines.is_empty() {
            line("CHECKS: none measured".into());
        } else {
            line("CHECKS (spec target and tolerance; before → after):".into());
            for l in &check_lines {
                line(format!("  {l}"));
            }
        }
        if !compared {
            line("VOLTAGES: not compared (no simulation values on one side)".into());
        } else if rows.is_empty() {
            line(format!("VOLTAGES: no net moved by more than 1% and 1 mV ({still} nets compared)"));
        } else {
            line("VOLTAGES (nets that moved, largest change first; before → after):".into());
            for r in &rows {
                line(format!("  {}", r.text));
            }
            let mut rest = Vec::new();
            if left_out > 0 {
                rest.push(format!("{left_out} more nets moved less"));
            }
            if still > 0 {
                rest.push(format!("{still} nets did not move"));
            }
            if !rest.is_empty() {
                line(format!("  ({})", rest.join("; ")));
            }
        }
        if !currents.is_empty() {
            line("CURRENTS (edited parts, operating point, into each pin; before → after):".into());
            for l in &currents {
                line(format!("  {l}"));
            }
        }
        for l in &erc {
            line(l.clone());
        }
        if !hazards.is_empty() {
            line(format!("SAFETY: mains-powered parts in this circuit: {}", hazards.join(", ")));
        }
        let tokens = out.chars().count().div_ceil(4);
        if tokens <= CONTEXT_TOKENS || rows.is_empty() {
            return ChangeContext {
                text: out,
                parts: edit.parts,
                blocks: edit.blocks,
                nets: rows.into_iter().map(|r| r.id).collect(),
                checks_moved,
                hazards,
                tokens: tokens as u32,
            };
        }
        rows.pop();
        left_out += 1;
    }
}

// ---------------------------------------------------------------- the edit

struct Edit {
    lines: Vec<String>,
    parts: Vec<RefDes>,
    blocks: Vec<BlockId>,
}

fn members(c: &Circuit, block: &BlockId) -> Vec<RefDes> {
    let mut v: Vec<RefDes> =
        c.parts.values().filter(|p| p.block.as_ref() == Some(block)).map(|p| p.refdes.clone()).collect();
    v.sort_by_key(|r| refdes_sort_key(r));
    v
}

/// A part as one row: `resistor_th [b2] resistance=18kΩ 1:B1_OUT 2:B2_N_A`.
fn part_row(c: &Circuit, reg: &Registry, r: &RefDes, pin_net: &BTreeMap<&PinRef, &NetId>) -> String {
    let inst = &c.parts[r];
    let mut row = inst.part.clone();
    if let Some(b) = &inst.block {
        row.push_str(&format!(" [{b}]"));
    }
    for (k, q) in &inst.params {
        row.push_str(&format!(" {k}={}", q.display));
    }
    for p in reg.part(&inst.part).map(|d| d.pins.as_slice()).unwrap_or_default() {
        let net = pin_net.get(&PinRef::new(r, &p.name)).map(|n| n.as_str()).unwrap_or("-");
        row.push_str(&format!(" {}:{net}", p.name));
    }
    row
}

fn edit(b: &Circuit, a: &Circuit, reg: &Registry) -> Edit {
    let mut lines = Vec::new();
    let mut parts: BTreeSet<RefDes> = BTreeSet::new();
    let mut blocks: IndexSet<BlockId> = IndexSet::new();
    let (pins_b, pins_a) = (b.pin_index(), a.pin_index());

    // Whole blocks added or removed are one line each, not one per part.
    let added: Vec<&BlockId> = a.blocks.keys().filter(|id| !b.blocks.contains_key(*id)).collect();
    let removed: Vec<&BlockId> = b.blocks.keys().filter(|id| !a.blocks.contains_key(*id)).collect();
    for (c, ids, verb) in [(a, &added, "added"), (b, &removed, "removed")] {
        for id in ids {
            let blk = &c.blocks[*id];
            let what = blk.template.as_deref().map(|t| format!(" template {t}")).unwrap_or_default();
            let m = members(c, id);
            lines.push(format!("block {id} \"{}\"{what} {verb}, with {}", blk.title, m.join(" ")));
            parts.extend(m);
            blocks.insert((*id).clone());
        }
    }
    let in_block =
        |c: &Circuit, r: &RefDes, ids: &[&BlockId]| c.parts[r].block.as_ref().is_some_and(|x| ids.contains(&x));

    // A net renamed, with the same pins, is one line; its pins have not moved.
    let pinset = |c: &Circuit, id: &NetId| c.nets[id].pins.iter().cloned().collect::<BTreeSet<PinRef>>();
    let mut renamed: BTreeMap<&NetId, &NetId> = BTreeMap::new();
    let gone: Vec<&NetId> = b.nets.keys().filter(|id| !a.nets.contains_key(*id)).collect();
    let new: Vec<&NetId> = a.nets.keys().filter(|id| !b.nets.contains_key(*id)).collect();
    for old in &gone {
        let pins = pinset(b, old);
        if pins.is_empty() {
            continue;
        }
        if let Some(to) = new.iter().find(|n| pinset(a, n) == pins) {
            renamed.insert(old, to);
            lines.push(format!("net {old} renamed {to}"));
        }
    }

    let mut all: Vec<&RefDes> = a.parts.keys().chain(b.parts.keys().filter(|r| !a.parts.contains_key(*r))).collect();
    all.sort_by_key(|r| refdes_sort_key(r));
    for r in all {
        match (b.parts.get(r), a.parts.get(r)) {
            (None, Some(_)) => {
                if !in_block(a, r, &added) {
                    lines.push(format!("{r} added: {}", part_row(a, reg, r, &pins_a)));
                    parts.insert(r.clone());
                }
            }
            (Some(_), None) => {
                if !in_block(b, r, &removed) {
                    lines.push(format!("{r} removed (was {})", part_row(b, reg, r, &pins_b)));
                    parts.insert(r.clone());
                }
            }
            (Some(pb), Some(pa)) => {
                let mut what = Vec::new();
                if pb.part != pa.part {
                    what.push(format!("swapped {} → {}", pb.part, pa.part));
                }
                let keys: BTreeSet<&String> = pb.params.keys().chain(pa.params.keys()).collect();
                for k in keys {
                    let (x, y) = (pb.params.get(k), pa.params.get(k));
                    if x.map(|q| q.si) != y.map(|q| q.si) {
                        let show =
                            |q: Option<&crate::units::Quantity>| q.map_or("-".to_string(), |q| q.display.clone());
                        what.push(format!("{k} {} → {}", show(x), show(y)));
                    }
                }
                let mut names: IndexSet<&str> = IndexSet::new();
                for part in [&pb.part, &pa.part] {
                    names.extend(reg.part(part).into_iter().flat_map(|d| d.pins.iter().map(|p| p.name.as_str())));
                }
                for p in names {
                    let pin = PinRef::new(r, p);
                    let x = pins_b.get(&pin).map(|n| *renamed.get(*n).unwrap_or(n));
                    let y = pins_a.get(&pin).copied();
                    if x != y {
                        let show = |n: Option<&NetId>| n.map_or("-".to_string(), |n| n.clone());
                        what.push(format!("pin {p} {} → {}", show(x), show(y)));
                    }
                }
                if !what.is_empty() {
                    let blk = pa.block.as_ref().map(|b| format!(" [{b}]")).unwrap_or_default();
                    lines.push(format!("{r}{blk}: {}", what.join("; ")));
                    parts.insert(r.clone());
                }
            }
            (None, None) => unreachable!("every refdes is in one of the circuits"),
        }
    }
    if b.analyses != a.analyses {
        lines.push(format!("analyses: {} → {}", analyses(&b.analyses), analyses(&a.analyses)));
    }
    if lines.is_empty() {
        lines.push("nothing electrical (moved or pinned on the schematic only)".into());
    }

    let mut parts: Vec<RefDes> = parts.into_iter().collect();
    parts.sort_by_key(|r| refdes_sort_key(r));
    for r in &parts {
        if let Some(id) = a.parts.get(r).or_else(|| b.parts.get(r)).and_then(|p| p.block.as_ref()) {
            blocks.insert(id.clone());
        }
    }
    let index = |id: &BlockId| a.blocks.get_index_of(id).unwrap_or(usize::MAX);
    let mut blocks: Vec<BlockId> = blocks.into_iter().collect();
    blocks.sort_by_key(|id| index(id));
    Edit { lines, parts, blocks }
}

/// `tran 5ms, ac 10Hz..100kHz`; an empty list is the core's automatic choice (LLD §8).
fn analyses(list: &[Analysis]) -> String {
    if list.is_empty() {
        return "automatic".into();
    }
    let each: Vec<String> = list
        .iter()
        .map(|a| match a {
            Analysis::Op => "op".to_string(),
            Analysis::Dc { source, start, stop, step } => format!(
                "dc {source} {}..{} step {}",
                format_sig(*start, Unit::Unitless),
                format_sig(*stop, Unit::Unitless),
                format_sig(*step, Unit::Unitless)
            ),
            Analysis::Ac { f_start, f_stop, .. } => {
                format!("ac {}..{}", format_sig(*f_start, Unit::Hertz), format_sig(*f_stop, Unit::Hertz))
            }
            Analysis::Tran { t_stop, .. } => format!("tran {}", format_sig(*t_stop, Unit::Second)),
        })
        .collect();
    each.join(", ")
}

// ---------------------------------------------------------------- values

fn moved(x: f64, y: f64, abs: f64) -> bool {
    let d = (x - y).abs();
    d > abs && d > REL * x.abs().max(y.abs())
}

/// How much a value moved, 0 to 1 (relative to the larger of the two); `None` when it did not.
/// A value on one side only counts as a whole change.
fn change(x: Option<f64>, y: Option<f64>, abs: f64) -> Option<f64> {
    match (x, y) {
        (Some(x), Some(y)) => moved(x, y, abs).then(|| (x - y).abs() / x.abs().max(y.abs())),
        (None, None) => None,
        _ => Some(1.0),
    }
}

fn check_moved(x: &CheckResult, y: &CheckResult) -> bool {
    x.pass != y.pass
        || match (x.measured, y.measured) {
            (Some(p), Some(q)) => moved(p, q, 0.0),
            (None, None) => false,
            _ => true,
        }
}

/// The checks before → after; those that did not move share one last line.
fn checks(before: &[CheckResult], after: &[CheckResult]) -> (Vec<String>, u32) {
    let key = |r: &CheckResult| (r.block.clone(), r.name.clone());
    let was: BTreeMap<(BlockId, String), &CheckResult> = before.iter().map(|r| (key(r), r)).collect();
    let now: BTreeSet<(BlockId, String)> = after.iter().map(key).collect();
    let result = |r: &CheckResult| match &r.measured_display {
        Some(m) => format!("{m} {}", if r.pass { "pass" } else { "FAIL" }),
        None => format!("not measured ({})", r.note.as_deref().unwrap_or("no result")),
    };
    let spec = |r: &CheckResult| format!("{} {} ({} ±{}%)", r.block, r.name, r.target_display, r.tol_pct);
    let (mut lines, mut same, mut n) = (Vec::new(), Vec::new(), 0u32);
    for r in after {
        match was.get(&key(r)) {
            Some(x) if !check_moved(x, r) => same.push(format!("{} {} {}", r.block, r.name, result(r))),
            Some(x) => {
                n += 1;
                lines.push(format!("{}: {} → {}", spec(r), result(x), result(r)));
            }
            None => {
                n += 1;
                lines.push(format!("{}: new, {}", spec(r), result(r)));
            }
        }
    }
    for x in before.iter().filter(|x| !now.contains(&key(x))) {
        n += 1;
        lines.push(format!("{}: no longer checked (was {})", spec(x), result(x)));
    }
    if !same.is_empty() {
        lines.push(format!("unchanged: {}", same.join("; ")));
    }
    (lines, n)
}

struct NetRow {
    id: NetId,
    score: f64,
    text: String,
}

/// Every net whose values moved, largest change first, and how many were compared and did not.
fn voltages(b: &Circuit, a: &Circuit, sb: &SimValues, sa: &SimValues) -> (Vec<NetRow>, usize) {
    let mut ids: BTreeSet<&str> = BTreeSet::new();
    for s in [sb, sa] {
        ids.extend(s.op_v.keys().map(String::as_str));
        ids.extend(s.ac.iter().flat_map(|ac| ac.v.keys().map(String::as_str)));
        ids.extend(s.tran.iter().flat_map(|t| t.v.keys().map(String::as_str)));
    }
    let hz = |s: &SimValues| s.ac.as_ref().map(|ac| ac.hz);
    let hz_moved = matches!((hz(sb), hz(sa)), (Some(x), Some(y)) if moved(x, y, 0.0));
    let opt = |x: Option<f64>| x.map_or("-".to_string(), v);
    let (mut rows, mut still) = (Vec::new(), 0);
    for id in ids {
        let (mut items, mut score) = (Vec::new(), 0f64);

        let (x, y) = (sb.op_v.get(id).copied(), sa.op_v.get(id).copied());
        if let Some(s) = change(x, y, ABS_V) {
            score = score.max(s);
            items.push(format!("op {} → {}", opt(x), opt(y)));
        }

        type Ac = (f64, Option<f64>, f64);
        let ac = |s: &SimValues| -> Option<Ac> {
            s.ac.as_ref().and_then(|ac| ac.v.get(id).map(|m| (*m, ac.deg.get(id).copied(), ac.hz)))
        };
        let (x, y) = (ac(sb), ac(sa));
        let ac_score = match (x, y) {
            (Some((mx, dx, _)), Some((my, dy, _))) => {
                let mag = change(Some(mx), Some(my), ABS_V);
                let phase = match (dx, dy) {
                    (Some(p), Some(q)) => {
                        let d = ((q - p + 180.0).rem_euclid(360.0) - 180.0).abs();
                        (d > ABS_DEG).then_some(d / 180.0)
                    }
                    _ => None,
                };
                (mag.is_some() || phase.is_some() || hz_moved)
                    .then(|| mag.unwrap_or(0.0).max(phase.unwrap_or(0.0)).max(if hz_moved { 1.0 } else { 0.0 }))
            }
            (None, None) => None,
            _ => Some(1.0),
        };
        if let Some(s) = ac_score {
            score = score.max(s);
            let side = |s: Option<Ac>, with_hz: bool| match s {
                None => "-".to_string(),
                Some((m, d, f)) => {
                    let mut t = v(m);
                    if let Some(d) = d {
                        t.push_str(&format!(" {}°", format_sig(d, Unit::Unitless)));
                    }
                    if with_hz {
                        t.push_str(&format!(" @{}", format_sig(f, Unit::Hertz)));
                    }
                    t
                }
            };
            items.push(match (x, y) {
                (Some((_, _, f)), Some(_)) if !hz_moved => {
                    format!("ac {} → {} @{}", side(x, false), side(y, false), format_sig(f, Unit::Hertz))
                }
                _ => format!("ac {} → {}", side(x, true), side(y, true)),
            });
        }

        let tran = |s: &SimValues| s.tran.as_ref().and_then(|t| t.v.get(id).copied());
        let (x, y) = (tran(sb), tran(sa));
        let tran_score = match (x, y) {
            (Some(p), Some(q)) => {
                let lo = change(Some(p.min), Some(q.min), ABS_V);
                let hi = change(Some(p.max), Some(q.max), ABS_V);
                (lo.is_some() || hi.is_some()).then(|| lo.unwrap_or(0.0).max(hi.unwrap_or(0.0)))
            }
            (None, None) => None,
            _ => Some(1.0),
        };
        if let Some(s) = tran_score {
            score = score.max(s);
            let side = |s: Option<super::Span>| s.map_or("-".to_string(), |s| format!("{}..{}", v(s.min), v(s.max)));
            items.push(format!("tran {} → {}", side(x), side(y)));
        }

        if items.is_empty() {
            still += 1;
            continue;
        }
        let tag = match (b.nets.contains_key(id), a.nets.contains_key(id)) {
            (false, true) => " (new net)",
            (true, false) => " (net removed)",
            _ => "",
        };
        rows.push(NetRow { id: id.to_string(), score, text: format!("{id}{tag}: {}", items.join(" | ")) });
    }
    rows.sort_by(|p, q| q.score.total_cmp(&p.score).then_with(|| p.id.cmp(&q.id)));
    (rows, still)
}

/// The edited parts' operating-point currents before → after: a two-pin part's one current,
/// else each pin's.
fn currents(parts: &[RefDes], b: &Circuit, a: &Circuit, reg: &Registry, sb: &SimValues, sa: &SimValues) -> Vec<String> {
    let opt = |x: Option<f64>| x.map_or("-".to_string(), |x| format_sig(x, Unit::Ampere));
    let mut lines = Vec::new();
    for r in parts {
        let Some(def) = a.parts.get(r).or_else(|| b.parts.get(r)).and_then(|p| reg.part(&p.part)) else { continue };
        let mut items = Vec::new();
        for p in &def.pins {
            let key = format!("{r}.{}", p.name);
            let (x, y) = (sb.op_i.get(&key).copied(), sa.op_i.get(&key).copied());
            if x.is_none() && y.is_none() {
                continue;
            }
            let same = if change(x, y, ABS_I).is_none() { " (no change)" } else { "" };
            items.push(format!("{}: {} → {}{same}", p.name, opt(x), opt(y)));
            if def.pins.len() == 2 {
                break;
            }
        }
        if !items.is_empty() {
            lines.push(format!("{r} into {}", items.join(", ")));
        }
        if lines.len() == MAX_CURRENT_PARTS {
            break;
        }
    }
    lines
}

/// ERC findings (user-edit rules) that the edit brought or cleared.
fn erc_lines(b: &Circuit, a: &Circuit, reg: &Registry) -> Vec<String> {
    let key = |i: &ErcIssue| (i.rule.clone(), i.message.clone());
    let (was, now) = (erc(b, reg, ErcContext::UserEdit, None), erc(a, reg, ErcContext::UserEdit, None));
    let (kb, ka): (BTreeSet<_>, BTreeSet<_>) = (was.iter().map(key).collect(), now.iter().map(key).collect());
    let appeared: Vec<&ErcIssue> = now.iter().filter(|i| !kb.contains(&key(i))).collect();
    let cleared: Vec<&ErcIssue> = was.iter().filter(|i| !ka.contains(&key(i))).collect();
    if appeared.is_empty() && cleared.is_empty() {
        return vec!["ERC: no change".into()];
    }
    let mut lines = Vec::new();
    for (label, list) in [("ERC new", &appeared), ("ERC resolved", &cleared)] {
        if list.is_empty() {
            continue;
        }
        lines.push(format!("{label}:"));
        for i in list.iter().take(MAX_ERC_LINES) {
            lines.push(format!("  {} {}: {}", i.rule, word(i.severity), i.message));
        }
        if list.len() > MAX_ERC_LINES {
            lines.push(format!("  and {} more", list.len() - MAX_ERC_LINES));
        }
    }
    lines
}
