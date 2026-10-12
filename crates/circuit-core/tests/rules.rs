//! Templates re-expressed as value rules (LLD §17 D5) choose exactly the part values their
//! former Rust solvers chose, which stay in `template::solvers` as the reference.

mod common;

use std::collections::BTreeMap;

use circuit_core::template::{Scale, verify_points};
use circuit_core::units::parse_quantity;

/// Template id -> the Rust solver its rules replace.
const REWRITTEN: [(&str, &str); 4] = [
    ("rc_lowpass", "rc_first_order"),
    ("rc_highpass", "rc_first_order"),
    ("noninverting_amp", "noninverting_amp"),
    ("sallen_key_lp", "sallen_key_lp"),
];

/// A small deterministic generator (no `rand` dependency): 64-bit LCG, top 53 bits.
struct Lcg(u64);

impl Lcg {
    fn next(&mut self) -> f64 {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        (self.0 >> 11) as f64 / (1u64 << 53) as f64
    }
}

#[test]
fn rules_choose_the_rust_solvers_values() {
    let reg = common::registry();
    let mut rng = Lcg(0x5eed);
    let mut compared = 0;
    for (id, solver) in REWRITTEN {
        let t = &reg.templates[id];
        assert!(t.rules.is_some() && t.solver.is_none(), "{id} is written as rules");
        let mut reference = t.clone();
        reference.rules = None;
        reference.solver = Some(solver.to_string());

        let mut cases: Vec<(BTreeMap<String, f64>, BTreeMap<String, f64>)> = verify_points(t)
            .into_iter()
            .map(|p| {
                let targets = p
                    .targets
                    .iter()
                    .map(|(k, v)| (k.clone(), parse_quantity(v, t.targets[k].unit).unwrap().si))
                    .collect();
                (targets, p.rails)
            })
            .collect();
        for _ in 0..200 {
            let targets = t
                .targets
                .iter()
                .map(|(k, td)| {
                    let x = rng.next();
                    let v = match td.scale {
                        Scale::Lin => td.min + x * (td.max - td.min),
                        Scale::Log => td.min * (td.max / td.min).powf(x),
                    };
                    (k.clone(), v)
                })
                .collect();
            let rails = t
                .rails
                .iter()
                .map(|(k, r)| {
                    let (lo, hi) = r.range();
                    (k.clone(), lo + rng.next() * (hi - lo))
                })
                .collect();
            cases.push((targets, rails));
        }
        for (targets, rails) in &cases {
            let ours = t.solve(&reg.parts, targets, rails);
            let theirs = reference.solve(&reg.parts, targets, rails);
            assert_eq!(ours, theirs, "{id} at {targets:?} {rails:?}");
            compared += 1;
        }
    }
    assert_eq!(compared, 4 * 205);
}

#[test]
fn rules_are_checked_when_the_registry_loads() {
    let reg = common::registry();
    let mut t = reg.templates["rc_lowpass"].clone();
    let rules = t.rules.as_mut().unwrap();
    rules.solve.insert("R1".into(), "nearest(E24, tau*C1, 1k, 100k)".into());
    let errs = t.compile_rules(t.rules.as_ref().unwrap(), &reg.parts).unwrap_err();
    assert_eq!(errs, ["step R1: gives A·s^2/V, but R1 is in ohms"]);
}
