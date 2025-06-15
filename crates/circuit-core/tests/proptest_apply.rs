//! Property tests (LLD §15): for random op sequences, every op that applies
//! (1) leaves a circuit that passes `validate`, (2) is undone exactly by its inverse, and
//! (3) is redone exactly by the inverse of its inverse. Ops that fail must leave no trace.
//!
//! The strategy generates mostly coherent ops (so the interesting paths actually run) with a
//! steady trickle of invalid ones; `strategy_exercises_every_op_type` guards against drift.

mod common;

use circuit_core::ops::{Author, envelope_from_value};
use circuit_core::{Circuit, apply, apply_ops, validate};
use common::*;
use proptest::prelude::*;
use serde_json::{Value, json};

/// (refdes, compatible parts, pins, (param, good values))
type Slot = (&'static str, &'static [&'static str], &'static [&'static str], (&'static str, &'static [&'static str]));
const SLOTS: &[Slot] = &[
    ("R1", &["resistor_th"], &["1", "2"], ("resistance", &["1k", "4k7", "10k"])),
    ("R2", &["resistor_th"], &["1", "2"], ("resistance", &["220", "1meg"])),
    ("C1", &["cap_film", "cap_elec"], &["1", "2", "P", "N"], ("capacitance", &["1u", "220n"])),
    ("U1", &["opamp_tl072"], &["OUT_A", "INM_A", "INP_A", "VCC", "VEE", "OUT_B", "INP_B"], ("nope", &["1"])),
    ("V1", &["vsource_dc", "vsource_sine"], &["P", "N"], ("voltage", &["5", "-12"])),
    ("V2", &["vsource_dc", "vsource_sine"], &["P", "N"], ("amplitude", &["1", "2.5"])),
    ("D1", &["diode_1n4148", "led_red"], &["A", "K"], ("nope", &["1"])),
    ("L1", &["inductor"], &["1", "2"], ("inductance", &["1m", "10u"])),
];
const ANY_PARTS: &[&str] = &["resistor_th", "cap_film", "opamp_tl072", "vsource_dc", "nonexistent"];
const NETS: &[&str] = &["GND", "N1", "N2", "VCC", "n1", "gnd"];
const BLOCKS: &[&str] = &["b1", "b2"];
const BAD_VALUES: &[&str] = &["bad", "1e12", "-1", "10uF"];

fn pick<T: Clone + std::fmt::Debug + 'static>(xs: &'static [T]) -> impl Strategy<Value = T> {
    prop::sample::select(xs)
}

fn slot() -> impl Strategy<Value = Slot> {
    pick(SLOTS)
}

fn refdes() -> impl Strategy<Value = &'static str> {
    prop_oneof![9 => slot().prop_map(|s| s.0), 1 => Just("X1")]
}

fn pin() -> impl Strategy<Value = String> {
    prop_oneof![
        9 => slot().prop_flat_map(|s| pick(s.2).prop_map(move |p| format!("{}.{p}", s.0))),
        1 => Just("R1.Z".to_string()),
    ]
}

fn value(s: Slot) -> impl Strategy<Value = String> {
    prop_oneof![4 => pick(s.3.1).prop_map(str::to_string), 1 => pick(BAD_VALUES).prop_map(str::to_string)]
}

fn kind() -> impl Strategy<Value = Value> {
    prop_oneof![
        6 => Just(Value::Null),
        1 => Just(json!({"kind": "signal"})),
        1 => Just(json!({"kind": "ground"})),
        2 => Just(json!({"kind": "power", "volts": 12.0})),
    ]
}

fn hint() -> impl Strategy<Value = Value> {
    prop_oneof![
        Just(json!({"kind": "flow", "dir": "right"})),
        (refdes(), refdes()).prop_map(|(a, b)| json!({"kind": "near", "a": a, "b": b})),
        pick(BLOCKS).prop_map(|b| json!({"kind": "group", "block": b})),
    ]
}

fn part_add() -> impl Strategy<Value = Value> {
    let coherent = slot().prop_flat_map(|s| {
        (pick(s.1), proptest::option::of(value(s)), proptest::option::of(pick(BLOCKS))).prop_map(move |(p, v, b)| {
            let params = v.map(|v| json!({ s.3.0: v })).unwrap_or(json!({}));
            let mut body = json!({"refdes": s.0, "part": p, "params": params});
            if let Some(b) = b {
                body["block"] = json!(b);
            }
            op("part.add", body)
        })
    });
    let random = (refdes(), pick(ANY_PARTS)).prop_map(|(r, p)| op("part.add", json!({"refdes": r, "part": p})));
    prop_oneof![8 => coherent, 1 => random]
}

fn any_op() -> impl Strategy<Value = Value> {
    prop_oneof![
        5 => part_add(),
        1 => refdes().prop_map(|r| op("part.remove", json!({"refdes": r}))),
        2 => slot().prop_flat_map(|s| value(s).prop_map(move |v|
            op("part.set_param", json!({"refdes": s.0, "key": s.3.0, "value": v})))),
        1 => slot().prop_flat_map(|s| pick(s.1).prop_map(move |p| op("part.swap", json!({"refdes": s.0, "part": p})))),
        1 => (refdes(), any::<bool>(), -100i32..100).prop_map(|(r, some, x)| {
            let placement = if some { json!({"x": x as f64, "y": 0.0, "rot": 90}) } else { Value::Null };
            op("part.pin", json!({"refdes": r, "placement": placement}))
        }),
        8 => (pick(NETS), prop::collection::vec(pin(), 1..4), kind()).prop_map(|(n, pins, k)| {
            let mut body = json!({"net": n, "pins": pins});
            if !k.is_null() {
                body["kind"] = k;
            }
            op("net.connect", body)
        }),
        1 => (pick(NETS), prop::collection::vec(pin(), 1..3))
            .prop_map(|(n, pins)| op("net.disconnect", json!({"net": n, "pins": pins}))),
        // Resolved against the live circuit by `resolve`: net #i, its first k pins.
        3 => (any::<usize>(), 1usize..3).prop_map(|(i, k)| op("net.disconnect", json!({"$net": i, "$pins": k}))),
        1 => (pick(NETS), pick(NETS)).prop_map(|(a, b)| op("net.rename", json!({"from": a, "to": b}))),
        2 => (any::<usize>(), pick(NETS)).prop_map(|(i, to)| op("net.rename", json!({"$net": i, "to": to}))),
        2 => (pick(BLOCKS), pick(NETS)).prop_map(|(b, n)| op("block.begin", json!({
            "id": b, "role": "filter", "title": "t",
            "spec": {"fc_hz": {"target": 1000.0, "tol_pct": 10.0}},
            "ports": [{"name": "in", "direction": "input", "net": n}]}))),
        1 => pick(BLOCKS).prop_map(|b| op("block.commit", json!({"id": b}))),
        1 => pick(BLOCKS).prop_map(|b| op("block.abort", json!({"id": b, "reason": "r"}))),
        1 => pick(BLOCKS).prop_map(|b| op("block.remove", json!({"id": b}))),
        1 => prop_oneof![
            Just(json!([{"type": "op"}])),
            Just(json!([{"type": "dc", "source": "V1", "start": 0.0, "stop": 5.0, "step": 0.5}])),
            Just(json!([{"type": "tran", "t_step": 1e-6, "t_stop": 1e-3}])),
        ]
        .prop_map(|a| op("analysis.set", json!({"analyses": a}))),
        1 => hint().prop_map(|h| op("hint.add", json!({"hint": h}))),
        1 => hint().prop_map(|h| op("hint.remove", json!({"hint": h}))),
        1 => Just(op("narrate", json!({"refs": ["R1"], "text": "hi"}))),
    ]
}

fn ops_strategy() -> impl Strategy<Value = Vec<(Value, bool)>> {
    prop::collection::vec((any_op(), prop::bool::weighted(0.3)), 1..80)
}

/// Turn index placeholders (`$net`, `$pins`) into names that exist in `c`.
fn resolve(c: &Circuit, o: &Value) -> Value {
    let mut o = o.clone();
    let Some(i) = o["body"]["$net"].as_u64() else { return o };
    if c.nets.is_empty() {
        return op("narrate", json!({"refs": [], "text": "no nets yet"}));
    }
    let net = &c.nets[i as usize % c.nets.len()];
    let body = o["body"].as_object_mut().unwrap();
    body.remove("$net");
    if let Some(k) = body.remove("$pins") {
        body.insert("net".into(), json!(net.id));
        let pins: Vec<String> = net.pins.iter().take(k.as_u64().unwrap() as usize).map(|p| p.to_string()).collect();
        body.insert("pins".into(), json!(pins));
    } else {
        body.insert("from".into(), json!(net.id));
    }
    o
}

fn envelope(c: &Circuit, author: &str, o: &Value) -> circuit_core::OpEnvelope {
    let mut v = resolve(c, o);
    v["v"] = json!(1);
    v["seq"] = json!(c.rev + 1);
    v["author"] = json!(author);
    v["base_rev"] = json!(c.rev);
    envelope_from_value(v).expect("strategy produces schema-valid ops")
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 512, ..ProptestConfig::default() })]

    #[test]
    fn apply_then_inverse_is_identity(ops in ops_strategy()) {
        let reg = registry();
        let mut cur = Circuit::new(reg.version.clone());
        for (o, by_llm) in &ops {
            let author = if *by_llm { "llm" } else { "user" };
            let Ok(a) = apply(&cur, &reg, &envelope(&cur, author, o)) else { continue };
            let problems = validate(&a.circuit, &reg);
            prop_assert!(problems.is_empty(), "after {o}: {problems:#?}");

            let (back, redo) = apply_ops(&a.circuit, &reg, &a.inverse, Author::User, None)
                .map_err(|e| TestCaseError::fail(format!("inverse of {o} failed: {e}")))?;
            prop_assert!(same_ir(&back, &cur), "undo of {o} did not restore the circuit");

            let (again, _) = apply_ops(&back, &reg, &redo, Author::User, None)
                .map_err(|e| TestCaseError::fail(format!("redo of {o} failed: {e}")))?;
            prop_assert!(same_ir(&again, &a.circuit), "redo of {o} differs");
            cur = a.circuit;
        }
    }
}

/// Guard against a vacuous property: every op type must actually succeed regularly.
#[test]
fn strategy_exercises_every_op_type() {
    use proptest::strategy::ValueTree;
    use proptest::test_runner::TestRunner;
    use std::collections::BTreeMap;

    let reg = registry();
    let mut runner = TestRunner::deterministic();
    let mut ok: BTreeMap<String, usize> = BTreeMap::new();
    for _ in 0..300 {
        let ops = ops_strategy().new_tree(&mut runner).unwrap().current();
        let mut cur = Circuit::new(reg.version.clone());
        for (o, by_llm) in &ops {
            let author = if *by_llm { "llm" } else { "user" };
            if let Ok(a) = apply(&cur, &reg, &envelope(&cur, author, o)) {
                *ok.entry(resolve(&cur, o)["op"].as_str().unwrap().to_string()).or_default() += 1;
                cur = a.circuit;
            }
        }
    }
    eprintln!("successful ops by type: {ok:?}");
    for name in [
        "part.add",
        "part.remove",
        "part.set_param",
        "part.swap",
        "part.pin",
        "net.connect",
        "net.disconnect",
        "net.rename",
        "block.begin",
        "block.commit",
        "block.abort",
        "block.remove",
        "analysis.set",
        "hint.add",
        "hint.remove",
        "narrate",
    ] {
        assert!(ok.get(name).copied().unwrap_or(0) >= 20, "{name} rarely succeeds: {ok:?}");
    }
}
