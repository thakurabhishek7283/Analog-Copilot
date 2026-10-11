//! Test plans and requirements in the IR (LLD §17 D8, D9). A test plan runs the same circuit
//! under several stimuli ("100 Hz, 1 kHz, 5 kHz at the same amplitude") and measures any nets; a
//! requirement is one numbered line of a design brief with the evidence that will prove it. Both
//! are set whole by `test.set` / `req.set`, so they are undoable and saved like analyses. What is
//! checked here is what can be checked when they are set; a scenario naming a part removed later
//! is reported when the plan runs, not refused then.

use std::collections::{BTreeMap, BTreeSet};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::ir::{Analysis, BlockId, Circuit, NetId};

pub const MAX_TESTS: usize = 16;
pub const MAX_SCENARIOS: usize = 16;
pub const MAX_PROBES: usize = 32;
pub const MAX_REQUIREMENTS: usize = 100;
pub const MAX_TEXT: usize = 500;

#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct TestPlan {
    /// `freq_compare`: lowercase [a-z0-9_].
    pub id: String,
    pub title: String,
    pub scenarios: Vec<Scenario>,
    pub probes: Vec<Probe>,
}

#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Scenario {
    pub name: String,
    /// Param overrides as `REFDES.param` -> value as typed (`"V2.frequency": "1k"`), applied to a
    /// scratch copy, so each scenario has its own netlist and cache entry.
    #[serde(default)]
    pub set: BTreeMap<String, String>,
    /// The analyses this scenario runs; absent, the circuit's own.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub analyses: Option<Vec<Analysis>>,
}

#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Probe {
    pub name: String,
    pub net: NetId,
    pub measure: Measure,
}

/// What a probe reads from its net in each scenario.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Measure {
    /// Half the peak-to-peak over the transient's second half.
    Amplitude,
    /// Average over the transient's second half (the DC level, bias included).
    Dc,
    /// |V(net)| / |V(input)| in the AC sweep at `at_hz`.
    AcGain {
        #[serde(rename = "in")]
        input: NetId,
        at_hz: f64,
    },
}

#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Requirement {
    /// `R1`, `R2`, ... as the brief numbers them.
    pub id: String,
    pub text: String,
    pub kind: RequirementKind,
    /// What will prove it; met, missing or unverifiable is decided from these after simulation.
    #[serde(default)]
    pub evidence: Vec<Evidence>,
}

#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum RequirementKind {
    Functional,
    Electrical,
    Constraint,
    Deliverable,
    Test,
}

#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Evidence {
    /// A block's spec check (`b2.fc_hz`).
    Check { block: BlockId, check: String },
    /// A probe of a test plan in one scenario, against a target.
    Probe { test: String, scenario: String, probe: String, target: f64, tol_pct: f64 },
    /// A fact about the circuit itself ("every op-amp is an LM358"), checked by the core's
    /// structure queries when they exist; until then it is unverifiable, never met.
    Structural { claim: String },
}

fn ident(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 48
        && s.as_bytes()[0].is_ascii_lowercase()
        && s.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
}

fn text_ok(s: &str) -> bool {
    !s.trim().is_empty() && s.chars().count() <= MAX_TEXT
}

/// Checks a param override (`refdes`, `param`, `value`) against the circuit.
pub type ParamCheck<'a> = &'a dyn Fn(&str, &str, &str) -> Result<(), String>;

/// Problems with a test-plan list. With `param` (when `test.set` sets it), also what it names in
/// the circuit as it is now: overridden params and probed nets. Without it (a loaded snapshot), only
/// its own shape, since a part or net it names may have gone since.
pub fn check_tests(c: &Circuit, tests: &[TestPlan], param: Option<ParamCheck>) -> Vec<String> {
    let mut errs = Vec::new();
    if tests.len() > MAX_TESTS {
        errs.push(format!("at most {MAX_TESTS} test plans"));
    }
    let mut ids = BTreeSet::new();
    for t in tests {
        let at = &t.id;
        if !ident(at) || !ids.insert(at) {
            errs.push(format!("test {at}: ids are unique lowercase [a-z0-9_]"));
        }
        if !text_ok(&t.title) {
            errs.push(format!("test {at}: title must be 1 to {MAX_TEXT} characters"));
        }
        if t.scenarios.is_empty() || t.scenarios.len() > MAX_SCENARIOS {
            errs.push(format!("test {at}: 1 to {MAX_SCENARIOS} scenarios"));
        }
        if t.probes.is_empty() || t.probes.len() > MAX_PROBES {
            errs.push(format!("test {at}: 1 to {MAX_PROBES} probes"));
        }
        let mut names = BTreeSet::new();
        for s in &t.scenarios {
            if !text_ok(&s.name) || !names.insert(&s.name) {
                errs.push(format!("test {at}: scenario names are unique and 1 to {MAX_TEXT} characters"));
            }
            for (key, value) in &s.set {
                let Some((refdes, p)) = key.split_once('.') else {
                    errs.push(format!("test {at}, {}: \"{key}\" is not REFDES.param", s.name));
                    continue;
                };
                if let Some(check) = param
                    && let Err(e) = check(refdes, p, value)
                {
                    errs.push(format!("test {at}, {}: {key}: {e}", s.name));
                }
            }
        }
        let mut names = BTreeSet::new();
        for p in &t.probes {
            if !ident(&p.name) || !names.insert(&p.name) {
                errs.push(format!("test {at}: probe names are unique lowercase [a-z0-9_]"));
            }
            let mut nets = vec![&p.net];
            if let Measure::AcGain { input, at_hz } = &p.measure {
                nets.push(input);
                if !(at_hz.is_finite() && *at_hz > 0.0) {
                    errs.push(format!("test {at}, probe {}: at_hz must be a positive frequency", p.name));
                }
            }
            for n in nets {
                if param.is_some() && !c.nets.contains_key(n) {
                    errs.push(format!("test {at}, probe {}: no net {n}", p.name));
                }
            }
        }
    }
    errs
}

/// Problems with a requirement list; with `refs`, also the checks and probes its evidence names.
pub fn check_requirements(c: &Circuit, reqs: &[Requirement], refs: bool) -> Vec<String> {
    let mut errs = Vec::new();
    if reqs.len() > MAX_REQUIREMENTS {
        errs.push(format!("at most {MAX_REQUIREMENTS} requirements"));
    }
    let tests: BTreeMap<&str, _> = c.tests.iter().map(|t| (t.id.as_str(), t)).collect();
    let mut ids = BTreeSet::new();
    for r in reqs {
        let at = &r.id;
        let numbered = at.len() >= 2
            && at.starts_with('R')
            && at[1..].bytes().all(|b| b.is_ascii_digit())
            && !at[1..].starts_with('0');
        if !numbered || !ids.insert(at) {
            errs.push(format!("requirement {at}: ids are unique R1, R2, ..."));
        }
        if !text_ok(&r.text) {
            errs.push(format!("requirement {at}: text must be 1 to {MAX_TEXT} characters"));
        }
        for e in &r.evidence {
            match e {
                Evidence::Check { block, check } => {
                    if refs && !c.blocks.get(block).is_some_and(|b| b.spec.contains_key(check)) {
                        errs.push(format!("requirement {at}: no check {block}.{check}"));
                    }
                }
                Evidence::Probe { test, scenario, probe, target, tol_pct } => {
                    let found = tests.get(test.as_str()).is_some_and(|t| {
                        t.scenarios.iter().any(|s| &s.name == scenario) && t.probes.iter().any(|p| &p.name == probe)
                    });
                    if refs && !found {
                        errs.push(format!("requirement {at}: no probe {probe} in scenario {scenario} of test {test}"));
                    }
                    if !(target.is_finite() && tol_pct.is_finite() && *tol_pct > 0.0) {
                        errs.push(format!("requirement {at}: target must be finite and tol_pct > 0"));
                    }
                }
                Evidence::Structural { claim } => {
                    if !text_ok(claim) {
                        errs.push(format!("requirement {at}: a claim is 1 to {MAX_TEXT} characters"));
                    }
                }
            }
        }
    }
    errs
}
