//! Components with params, laws and imported SPICE models (LLD §17 D3–D5, D10): the 12 reference
//! black boxes in `tools/sim/blackboxes.yaml` load and their benches compile at every
//! characterisation point; value-specialised sub-circuits; value rules on a composite;
//! behavioral `require`s measured after a transient; what each kind refuses.

mod common;

use std::collections::BTreeMap;

use circuit_core::characterise::{SimOutcome, Trust};
use circuit_core::library::ComponentDef;
use circuit_core::ops::{Author, LibAdd, Op};
use circuit_core::{Circuit, CompileOpts, ErrorCode, apply_ops, compile};
use common::*;
use serde_json::{Value, json};

/// The black boxes, `@path` model texts read from the repository.
fn blackboxes() -> Vec<(String, ComponentDef)> {
    let root = registry_root().parent().unwrap().to_path_buf();
    let text = std::fs::read_to_string(root.join("tools/sim/blackboxes.yaml")).unwrap();
    let docs: serde_norway::Value = serde_norway::from_str(&text).unwrap();
    let mut out = Vec::new();
    for (k, v) in docs.as_mapping().unwrap() {
        let mut v: Value = serde_json::to_value(v).unwrap();
        if let Some(t) = v.pointer("/model/text").and_then(Value::as_str)
            && let Some(path) = t.strip_prefix('@')
        {
            v["model"]["text"] = json!(std::fs::read_to_string(root.join(path)).unwrap());
        }
        let def: ComponentDef = serde_json::from_value(v).unwrap_or_else(|e| panic!("{k:?}: {e}"));
        out.push((k.as_str().unwrap().to_string(), def));
    }
    out
}

fn with_lib(defs: &[ComponentDef]) -> Circuit {
    let reg = registry();
    let ops: Vec<Op> = defs.iter().map(|d| Op::LibAdd(LibAdd { def: Box::new(d.clone()) })).collect();
    apply_ops(&Circuit::new(reg.version.clone()), &reg, &ops, Author::User, None).unwrap_or_else(|e| panic!("{e}")).0
}

#[test]
fn every_black_box_loads_and_its_benches_compile() {
    let reg = registry();
    let boxes = blackboxes();
    assert_eq!(boxes.len(), 12);
    for (name, def) in &boxes {
        let c = with_lib(std::slice::from_ref(def));
        let runs = circuit_core::characterise::benches(&reg, &c.library, &def.part.id).unwrap();
        let ranged = def.part.params.values().filter(|p| p.min.zip(p.max).is_some_and(|(a, b)| a < b)).count();
        assert_eq!(runs.len(), if ranged == 0 { 1 } else { 5 }, "{name}");
        for r in &runs {
            let n = r.netlist.as_ref().unwrap_or_else(|| panic!("{name} at {:?}: {:?}", r.params, r.error));
            let checks = &def.contract.as_ref().unwrap().checks;
            assert_eq!(n.checks.len(), checks.len(), "{name}");
            for ch in &n.checks {
                assert!(ch.missing.is_none(), "{name} {}: {:?}\n{}", ch.name, ch.missing, n.text);
            }
            assert!(
                n.text.lines().all(|l| !l.to_ascii_lowercase().starts_with(".include") || l.contains("models/")),
                "{name}"
            );
        }
        // A report from made-up measurements: nothing measured is a draft.
        let outcomes = vec![SimOutcome::default(); runs.len()];
        let rep = circuit_core::characterise::report(&reg, &c.library, &def.part.id, "47", &runs, &outcomes).unwrap();
        assert_eq!(rep.trust, Trust::Draft, "{name}");
        assert_eq!(rep.def_hash.len(), 64);
    }
}

fn opamp_law() -> ComponentDef {
    blackboxes().into_iter().find(|(k, _)| k == "lm358_law").unwrap().1
}

#[test]
fn each_param_set_is_its_own_subckt() {
    let reg = registry();
    let def = opamp_law();
    let c = with_lib(&[def]);
    let c = run(
        &c,
        &reg,
        "user",
        &[
            op("part.add", json!({"refdes": "V1", "part": "vsource_dc", "params": {"voltage": "5"}})),
            op("part.add", json!({"refdes": "U1", "part": "lib:lm358_law"})),
            op("part.add", json!({"refdes": "U2", "part": "lib:lm358_law"})),
            op("part.add", json!({"refdes": "U3", "part": "lib:lm358_law", "params": {"gbw": "2meg"}})),
            op("net.connect", json!({"net": "VCC", "pins": ["V1.P", "U1.VCC", "U2.VCC", "U3.VCC"]})),
            op(
                "net.connect",
                json!({"net": "GND", "pins": ["V1.N", "U1.VEE", "U2.VEE", "U3.VEE", "U1.INP", "U2.INP", "U3.INP"]}),
            ),
            op("net.connect", json!({"net": "A", "pins": ["U1.OUT", "U1.INM"]})),
            op("net.connect", json!({"net": "B", "pins": ["U2.OUT", "U2.INM"]})),
            op("net.connect", json!({"net": "C", "pins": ["U3.OUT", "U3.INM"]})),
            op("analysis.set", json!({"analyses": [{"type": "tran", "t_step": 1e-6, "t_stop": 1e-3}]})),
        ],
    );
    let n = compile(&c, &reg, &CompileOpts::default()).unwrap();
    let subs: Vec<&str> = n.text.lines().filter(|l| l.starts_with(".subckt")).collect();
    assert_eq!(subs.len(), 2, "two distinct value sets\n{}", n.text);
    assert!(subs.iter().all(|l| l.starts_with(".subckt lib_lm358_law_")), "{subs:?}");
    let x: Vec<&str> = n.text.lines().filter(|l| l.starts_with("XU")).collect();
    assert_eq!(x[0].split_whitespace().last(), x[1].split_whitespace().last(), "U1 and U2 share one");
    assert_ne!(x[0].split_whitespace().last(), x[2].split_whitespace().last(), "U3 has its own");
    // Each instance's require, measured over the transient.
    assert_eq!(n.envelope.len(), 3);
    assert!(n.text.contains(".meas tran env_u1_5_hi max v(xu1.bx_q5)"), "{}", n.text);
    let meas: BTreeMap<String, f64> =
        [("env_u1_5_hi", 5.0), ("env_u1_5_lo", 5.0), ("env_u2_5_hi", 40.0), ("env_u2_5_lo", 5.0)]
            .into_iter()
            .map(|(k, v)| (k.to_string(), v))
            .collect();
    let r = circuit_core::spice::evaluate_envelope(&n.envelope, &meas);
    assert!(r[0].pass && !r[1].pass && r[2].pass, "{r:#?}");
    assert_eq!(r[1].note, "from 5V to 40V");
    // A black box has nothing to expand.
    let e = try_op(&c, &reg, "user", &op("instance.expand", json!({"refdes": "U1"}))).unwrap_err();
    assert_eq!(e.code, ErrorCode::ComponentInvalid);
    assert!(e.message.contains("a black box"), "{}", e.message);
}

/// A parameterised gain stage: rules choose R1 and R2 for `gain`.
fn gain_stage() -> Value {
    json!({
        "part": {"id": "lib:gain_stage", "category": "X", "prefix": "U", "title": "Gain stage", "symbol": "auto",
                 "pins": [{"name": "IN", "num": 1, "type": "input"}, {"name": "OUT", "num": 2, "type": "output"},
                          {"name": "VCC", "num": 3, "type": "power_pos"}, {"name": "VEE", "num": 4, "type": "power_neg"}],
                 "params": {"gain": {"unit": "unitless", "default": "11", "min": 2, "max": 50}}},
        "version": 1,
        "model": {"kind": "composite",
                  "parts": {"R1": {"part": "resistor_th"}, "R2": {"part": "resistor_th"}, "U1": {"part": "opamp_tl072"}},
                  "nets": {"IN": ["U1.INP_A"], "N_M": ["R1.1", "R2.1", "U1.INM_A"], "OUT": ["R2.2", "U1.OUT_A"],
                           "GND": ["R1.2"], "VCC": ["U1.VCC"], "VEE": ["U1.VEE"]}},
        "rules": {"solve": {"ratio": "gain - 1", "R1": "search(E24, 1k, 100k)", "R2": "nearest(E24, R1*ratio, 100, 1meg)"},
                  "estimates": {"ratio": "R2/R1"}, "prefer": {"R1": "10k"}},
        "provenance": {"kind": "user"}
    })
}

#[test]
fn rules_set_a_composites_values_and_expand_with_them() {
    let reg = registry();
    let c = run(&Circuit::new(reg.version.clone()), &reg, "user", &[op("lib.add", json!({"def": gain_stage()}))]);
    let c = run(
        &c,
        &reg,
        "user",
        &[
            op("part.add", json!({"refdes": "U1", "part": "lib:gain_stage", "params": {"gain": "5"}})),
            op("part.add", json!({"refdes": "U2", "part": "lib:gain_stage"})),
        ],
    );
    let n = compile(&c, &reg, &CompileOpts::default()).unwrap();
    // gain 5: 7.5k and 30k (exactly 4); gain 11: 10k and 100k.
    assert!(n.text.contains("R1 n_m 0 7.5e3\nR2 n_m out 3e4"), "{}", n.text);
    assert!(n.text.contains("R2 n_m out 1e5"), "{}", n.text);
    let x = try_op(&c, &reg, "user", &op("instance.expand", json!({"refdes": "U2"}))).unwrap().circuit;
    let r2: Vec<_> =
        x.parts.values().filter(|p| p.part == "resistor_th").map(|p| p.params["resistance"].display.clone()).collect();
    assert!(r2.contains(&"100kΩ".to_string()) && r2.contains(&"10kΩ".to_string()), "{r2:?}");
    // A param an instance sets cannot disappear.
    let mut without = gain_stage();
    without["part"]["params"] = json!({});
    without["rules"] = Value::Null;
    without["model"]["parts"]["R2"]["params"] = json!({"resistance": "100k"});
    let e = try_op(&c, &reg, "user", &op("lib.update", json!({"id": "lib:gain_stage", "def": without}))).unwrap_err();
    assert!(e.message.contains("would lose param gain"), "{}", e.message);
}

#[test]
fn problems_are_listed_in_plain_words() {
    let reg = registry();
    let c0 = Circuit::new(reg.version.clone());
    let mut bad = opamp_law();
    if let circuit_core::library::Model::Behavioral(b) = &mut bad.model {
        b.laws[0] = "V(OUT) = V(INP) + I(OUT)".into();
        b.laws.push("R(INP) = V(INM)".into());
    }
    let e = try_op(&c0, &reg, "user", &op("lib.add", json!({"def": bad}))).unwrap_err();
    assert_eq!(e.code, ErrorCode::ComponentInvalid);
    assert!(e.message.contains("law 1: cannot add V(INP) (volts) and I(OUT) (amps)"), "{}", e.message);
    assert!(e.message.contains("law 6: R(INP) gives volts, but it must be in ohms"), "{}", e.message);

    let mut evil = blackboxes().into_iter().find(|(k, _)| k == "lm358_import").unwrap().1;
    if let circuit_core::library::Model::Spice(s) = &mut evil.model {
        s.text = format!(".include /etc/passwd\n{}", s.text);
    }
    let e = try_op(&c0, &reg, "user", &op("lib.add", json!({"def": evil}))).unwrap_err();
    assert!(e.message.contains("line 1: .include is not allowed"), "{}", e.message);

    let mut bad_rules = gain_stage();
    bad_rules["rules"]["solve"]["R2"] = json!("nearest(E24, R1*ratio*1s, 100, 1meg)");
    let e = try_op(&c0, &reg, "user", &op("lib.add", json!({"def": bad_rules}))).unwrap_err();
    assert!(e.message.contains("rules: step R2: gives henries, but R2 is in ohms"), "{}", e.message);
}

#[test]
fn a_require_on_known_rails_is_checked_by_erc() {
    let reg = registry();
    let c = with_lib(&[opamp_law()]);
    let rails = |v: &str| {
        run(
            &c,
            &reg,
            "user",
            &[
                op("part.add", json!({"refdes": "U1", "part": "lib:lm358_law"})),
                op(
                    "net.connect",
                    json!({"net": "VCC", "pins": ["U1.VCC"], "kind": {"kind": "power", "volts": v.parse::<f64>().unwrap()}}),
                ),
                op("net.connect", json!({"net": "GND", "pins": ["U1.VEE", "U1.INP"]})),
                op("net.connect", json!({"net": "A", "pins": ["U1.OUT", "U1.INM"]})),
            ],
        )
    };
    let issues = |c: &Circuit| circuit_core::erc(c, &reg, circuit_core::ErcContext::UserEdit, None);
    assert!(issues(&rails("5")).iter().all(|i| i.rule != "ERC008"), "{:#?}", issues(&rails("5")));
    let bad = issues(&rails("2"));
    let e = bad.iter().find(|i| i.rule == "ERC008").expect("ERC008");
    assert_eq!(e.message, "U1 needs V(VCC) - V(VEE) in [3, 32]; here it is 2V");
}
