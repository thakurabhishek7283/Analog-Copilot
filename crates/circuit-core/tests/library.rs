//! The project library (LLD §17 D1–D2, D6, D8–D9, D13, D15): component definitions embedded in the
//! circuit, nested instances, ERC on their pins' semantics, `.subckt` compilation, expansion,
//! test plans and requirements, and the v1 → v2 migration.

mod common;

use circuit_core::erc::{ErcCode, ErcContext};
use circuit_core::ir::{BlockStatus, NetKind, PinRef, SCHEMA_VERSION};
use circuit_core::ops::{Author, Op};
use circuit_core::session::Session;
use circuit_core::{Circuit, CompileOpts, ErrorCode, apply_ops, compile, erc, validate};
use common::*;
use serde_json::{Value, json};
use std::sync::Arc;

fn pin(name: &str, num: u16, kind: &str) -> Value {
    json!({"name": name, "num": num, "type": kind})
}

/// An RC low-pass: IN -R1- OUT -C1- COM.
fn rc_lp(r: &str) -> Value {
    json!({
        "part": {"id": "lib:rc_lp", "category": "X", "title": "RC low-pass", "symbol": "auto",
                 "pins": [pin("IN", 1, "input"), pin("OUT", 2, "output"), pin("COM", 3, "passive")]},
        "version": 1,
        "model": {"kind": "composite",
                  "parts": {"R1": {"part": "resistor_th", "params": {"resistance": r}},
                            "C1": {"part": "cap_film", "params": {"capacitance": "100n"}}},
                  "nets": {"IN": ["R1.1"], "OUT": ["R1.2", "C1.1"], "COM": ["C1.2"]}},
        "provenance": {"kind": "user"}
    })
}

/// A TL072 follower.
fn buf() -> Value {
    json!({
        "part": {"id": "lib:buf", "category": "X", "prefix": "U", "title": "Buffer", "symbol": "auto",
                 "pins": [pin("IN", 1, "input"), pin("OUT", 2, "output"), pin("VCC", 3, "power_pos"), pin("VEE", 4, "power_neg")]},
        "version": 1,
        "model": {"kind": "composite",
                  "parts": {"U1": {"part": "opamp_tl072"}},
                  "nets": {"IN": ["U1.INP_A"], "OUT": ["U1.OUT_A", "U1.INM_A"], "VCC": ["U1.VCC"], "VEE": ["U1.VEE"]}},
        "provenance": {"kind": "user"}
    })
}

/// Both nested: the filter's common end on the circuit's ground, then the buffer.
fn stage() -> Value {
    json!({
        "part": {"id": "lib:stage", "category": "X", "prefix": "XU", "title": "Filtered buffer", "symbol": "auto",
                 "pins": [pin("IN", 1, "input"), pin("OUT", 2, "output"), pin("VCC", 3, "power_pos"), pin("VEE", 4, "power_neg")]},
        "version": 1,
        "model": {"kind": "composite",
                  "parts": {"X1": {"part": "lib:rc_lp"}, "U1": {"part": "lib:buf"}},
                  "nets": {"IN": ["X1.IN"], "N_M": ["X1.OUT", "U1.IN"], "OUT": ["U1.OUT"],
                           "VCC": ["U1.VCC"], "VEE": ["U1.VEE"], "GND": ["X1.COM"]}},
        "provenance": {"kind": "llm", "job_id": "j1"}
    })
}

fn lib_ops() -> Vec<Value> {
    vec![
        op("lib.add", json!({"def": rc_lp("10k")})),
        op("lib.add", json!({"def": buf()})),
        op("lib.add", json!({"def": stage()})),
    ]
}

/// The ±12 V supply and a 1 kHz sine source (b2's output on B2_OUT), then the library and one
/// stage instance between the source and OUT.
fn with_stage() -> Circuit {
    let reg = registry();
    let c = run(&Circuit::new(reg.version.clone()), &reg, "llm", &supply_ops());
    let c = run(&c, &reg, "llm", &source_ops());
    let c = run(&c, &reg, "user", &lib_ops());
    run(
        &c,
        &reg,
        "user",
        &[
            op("part.add", json!({"refdes": "XU1", "part": "lib:stage"})),
            op("net.connect", json!({"net": "N_IN", "pins": ["XU1.IN"]})),
            op("net.connect", json!({"net": "OUT", "pins": ["XU1.OUT"]})),
            op("net.connect", json!({"net": "VCC", "pins": ["XU1.VCC"]})),
            op("net.connect", json!({"net": "VEE", "pins": ["XU1.VEE"]})),
            op("part.add", json!({"refdes": "R9", "part": "resistor_th", "params": {"resistance": "10k"}})),
            op("net.connect", json!({"net": "OUT", "pins": ["R9.1"]})),
            op("net.connect", json!({"net": "GND", "pins": ["R9.2"]})),
        ],
    )
}

fn source_net(c: &Circuit) -> &str {
    c.blocks["b2"].ports.iter().find(|p| p.name == "out").map(|p| p.net.as_str()).unwrap()
}

#[test]
fn definitions_are_checked_and_get_their_dc_groups() {
    let c = with_stage();
    let reg = registry();
    assert!(validate(&c, &reg).is_empty(), "{:#?}", validate(&c, &reg));
    assert_eq!(c.schema_version, SCHEMA_VERSION);
    assert_eq!(c.library.keys().collect::<Vec<_>>(), ["lib:buf", "lib:rc_lp", "lib:stage"], "sorted by id");
    let groups = |id: &str| -> Vec<(String, Option<u16>)> {
        c.library[id].part.pins.iter().map(|p| (p.name.clone(), p.dc_group)).collect()
    };
    // R1 joins IN and OUT; C1 blocks COM.
    assert_eq!(groups("lib:rc_lp"), [("IN".into(), Some(1)), ("OUT".into(), Some(1)), ("COM".into(), None)]);
    // An op-amp conducts between its output and supplies; its input is isolated.
    assert_eq!(
        groups("lib:buf"),
        [("IN".into(), None), ("OUT".into(), Some(1)), ("VCC".into(), Some(1)), ("VEE".into(), Some(1))]
    );
    // Nested: IN reaches only the buffer's input, so it conducts to no other pin.
    assert_eq!(
        groups("lib:stage"),
        [("IN".into(), None), ("OUT".into(), Some(1)), ("VCC".into(), Some(1)), ("VEE".into(), Some(1))]
    );
}

#[test]
fn a_bad_definition_lists_every_problem() {
    let reg = registry();
    let c = Circuit::new(reg.version.clone());
    let mut bad = rc_lp("10k");
    bad["model"]["nets"]["IN"] = json!(["R1.1", "R1.9"]);
    bad["model"]["nets"]["N_X"] = json!(["C9.1"]);
    bad["model"]["parts"]["Q1"] = json!({"part": "resistor_th"});
    let e = try_op(&c, &reg, "user", &op("lib.add", json!({"def": bad}))).unwrap_err();
    assert_eq!(e.code, ErrorCode::ComponentInvalid);
    for want in [
        "R1 has pins 1 2",
        "C9.1 names no part",
        "Q1: a resistor_th needs a refdes like R1",
        "internal net needs at least 2 pins",
    ] {
        assert!(e.message.contains(want), "missing \"{want}\" in: {}", e.message);
    }
    // An internal node only capacitors reach would make the simulation singular.
    let mut floating = rc_lp("10k");
    floating["model"]["parts"]["C2"] = json!({"part": "cap_film"});
    floating["model"]["nets"]["N_F"] = json!(["C1.2", "C2.1"]);
    floating["model"]["nets"]["COM"] = json!(["C2.2"]);
    let e = try_op(&c, &reg, "user", &op("lib.add", json!({"def": floating}))).unwrap_err();
    assert!(e.message.contains("N_F has no DC path"), "{}", e.message);
    // A definition may only use definitions already in the library.
    let e = try_op(&c, &reg, "user", &op("lib.add", json!({"def": stage()}))).unwrap_err();
    assert!(e.message.contains("lib:rc_lp is not in the project library"), "{}", e.message);
    // Registry parts cannot claim category X; ids are namespaced.
    let mut named = rc_lp("10k");
    named["part"]["id"] = json!("rc_lp");
    let e = try_op(&c, &reg, "user", &op("lib.add", json!({"def": named}))).unwrap_err();
    assert!(e.message.contains("must be lib:"), "{}", e.message);
}

#[test]
fn instances_take_the_definitions_prefix() {
    let reg = registry();
    let c = run(&Circuit::new(reg.version.clone()), &reg, "user", &lib_ops());
    let e = try_op(&c, &reg, "user", &op("part.add", json!({"refdes": "U5", "part": "lib:stage"}))).unwrap_err();
    assert_eq!(e.code, ErrorCode::RefdesInvalid);
    assert!(e.message.contains("must start with XU"), "{}", e.message);
    let c = run(&c, &reg, "user", &[op("part.add", json!({"refdes": "XU5", "part": "lib:stage"}))]);
    assert_eq!(circuit_core::edit::next_refdes(&c, &reg, "lib:stage").unwrap(), "XU1");
    assert_eq!(circuit_core::edit::next_refdes(&c, &reg, "lib:rc_lp").unwrap(), "X1");
    let e = try_op(&c, &reg, "user", &op("part.add", json!({"refdes": "X1", "part": "lib:nope"}))).unwrap_err();
    assert_eq!(e.code, ErrorCode::PartNotInRegistry);
}

#[test]
fn nested_instances_compile_to_subcircuits() {
    let c = with_stage();
    let reg = registry();
    let n = compile(&c, &reg, &CompileOpts::default()).unwrap();
    let text = &n.text;
    for want in [
        ".subckt lib_buf in out vcc vee\nXU1_A in out vcc vee out TL072\n.ends lib_buf\n",
        ".subckt lib_rc_lp in out com\nC1 out com 1e-7\nR1 in out 1e4\n.ends lib_rc_lp\n",
        ".subckt lib_stage in out vcc vee\nXU1 n_m out vcc vee lib_buf\nXX1 in n_m 0 lib_rc_lp\n.ends lib_stage\n",
        &format!("XXU1 {} out vcc vee lib_stage\n", source_net(&c).to_ascii_lowercase()),
    ] {
        assert!(text.contains(want), "missing {want:?} in\n{text}");
    }
    assert!(text.find(".include models/tl072.lib").unwrap() < text.find(".subckt").unwrap());
    assert_eq!(n.leaf_devices, 3 + 1 + 3, "V1-V3 and R9 outside; R1, C1 and one TL072 unit inside");
    // No pin currents into a sub-circuit, as for model sub-circuits.
    assert!(!n.pin_currents.keys().any(|k| k.starts_with("XU1.")));
    // A circuit without a library compiles to the text it always did.
    let plain = run(&Circuit::new(reg.version.clone()), &reg, "llm", &supply_ops());
    assert!(!compile(&plain, &reg, &CompileOpts::default()).unwrap().text.contains(".subckt"));
}

#[test]
fn erc_reads_instances_through_their_pins() {
    let reg = registry();
    let c = with_stage();
    let issues = erc(&c, &reg, ErcContext::UserEdit, None);
    assert!(issues.is_empty(), "{issues:#?}");
    // Off its supply: the instance's power pins are checked like an IC's.
    let off = run(&c, &reg, "user", &[op("net.disconnect", json!({"net": "VCC", "pins": ["XU1.VCC"]}))]);
    let codes: Vec<ErcCode> = erc(&off, &reg, ErcContext::UserEdit, None).iter().map(|i| i.code).collect();
    assert!(codes.contains(&ErcCode::UnpoweredIc) && codes.contains(&ErcCode::FloatingPin), "{codes:?}");
    // OUT's DC path to ground is through the buffer (group 1, with the supplies): drop R9 and the
    // output is still tied down; drop the supplies too and it is not.
    let no_load = run(&c, &reg, "user", &[op("part.remove", json!({"refdes": "R9"}))]);
    assert!(!erc(&no_load, &reg, ErcContext::UserEdit, None).iter().any(|i| i.code == ErcCode::NoDcPath));
    let isolated = run(
        &no_load,
        &reg,
        "user",
        &[
            op("net.disconnect", json!({"net": "VCC", "pins": ["XU1.VCC"]})),
            op("net.disconnect", json!({"net": "VEE", "pins": ["XU1.VEE"]})),
        ],
    );
    let floating = circuit_core::erc::floating_nets(&isolated, &reg);
    assert_eq!(floating, ["OUT"]);
}

#[test]
fn update_reaches_users_and_undoes_exactly() {
    let reg = registry();
    let c = with_stage();
    let a = try_op(&c, &reg, "user", &op("lib.update", json!({"id": "lib:rc_lp", "def": rc_lp("4k7")}))).unwrap();
    assert!(a.patch.library_changed && a.patch.parts_upserted.is_empty());
    let text = compile(&a.circuit, &reg, &CompileOpts::default()).unwrap().text;
    assert!(text.contains("R1 in out 4.7e3"), "{text}");
    let (back, _) = apply_ops(&a.circuit, &reg, &a.inverse, Author::User, None).unwrap();
    assert!(same_ir(&back, &c));

    // Dropping a pin an instance or a user definition connects to is refused.
    let mut fewer = rc_lp("10k");
    fewer["part"]["pins"] = json!([pin("IN", 1, "input"), pin("OUT", 2, "output")]);
    fewer["model"]["nets"].as_object_mut().unwrap().remove("COM");
    fewer["model"]["nets"]["GND"] = json!(["C1.2"]);
    let e = try_op(&c, &reg, "user", &op("lib.update", json!({"id": "lib:rc_lp", "def": fewer}))).unwrap_err();
    assert!(e.message.contains("lib:stage, which uses lib:rc_lp"), "{}", e.message);

    // A definition cannot be made to use one of its own users.
    let mut looped = rc_lp("10k");
    looped["model"]["parts"]["XU1"] = json!({"part": "lib:stage"});
    looped["model"]["nets"]["N_L"] = json!(["XU1.IN", "XU1.OUT"]);
    let e = try_op(&c, &reg, "user", &op("lib.update", json!({"id": "lib:rc_lp", "def": looped}))).unwrap_err();
    assert!(e.message.contains("would make a loop"), "{}", e.message);
}

#[test]
fn remove_only_what_nothing_uses() {
    let reg = registry();
    let c = with_stage();
    let e = try_op(&c, &reg, "user", &op("lib.remove", json!({"id": "lib:rc_lp"}))).unwrap_err();
    assert_eq!(e.code, ErrorCode::ComponentInUse);
    assert!(e.message.contains("lib:stage"), "{}", e.message);
    let e = try_op(&c, &reg, "user", &op("lib.remove", json!({"id": "lib:stage"}))).unwrap_err();
    assert!(e.message.contains("XU1"), "{}", e.message);
    let gone = run(
        &c,
        &reg,
        "user",
        &[op("part.remove", json!({"refdes": "XU1"})), op("lib.remove", json!({"id": "lib:stage"}))],
    );
    assert!(!gone.library.contains_key("lib:stage"));
    let a = try_op(&gone, &reg, "user", &op("lib.remove", json!({"id": "lib:buf"}))).unwrap();
    let (back, _) = apply_ops(&a.circuit, &reg, &a.inverse, Author::User, None).unwrap();
    assert!(same_ir(&back, &gone), "the library is sorted, so undo restores its order");
}

#[test]
fn expand_turns_an_instance_into_a_block_and_undoes() {
    let reg = registry();
    let c = with_stage();
    let a = try_op(&c, &reg, "user", &op("instance.expand", json!({"refdes": "XU1"}))).unwrap();
    let x = &a.circuit;
    assert_eq!(x.rev, c.rev + 1, "one op, one rev");
    assert!(!x.parts.contains_key("XU1"));
    let block = &x.blocks["b3"];
    assert_eq!((block.title.as_str(), block.status), ("Filtered buffer", BlockStatus::Committed));
    // One level: the two nested components are instances in the circuit now, wired as inside.
    let inside: Vec<(&str, &str)> = x
        .parts
        .values()
        .filter(|p| p.block.as_deref() == Some("b3"))
        .map(|p| (p.refdes.as_str(), p.part.as_str()))
        .collect();
    assert_eq!(inside, [("X1", "lib:rc_lp"), ("U1", "lib:buf")]);
    assert_eq!(x.net_of(&PinRef::new("X1", "IN")).map(String::as_str), Some(source_net(&c)));
    assert_eq!(x.net_of(&PinRef::new("U1", "OUT")).map(String::as_str), Some("OUT"));
    assert_eq!(x.net_of(&PinRef::new("X1", "COM")).map(String::as_str), Some("GND"));
    assert_eq!(x.net_of(&PinRef::new("X1", "OUT")).map(String::as_str), Some("XU1_N_M"));
    assert_eq!(x.nets["VCC"].kind, NetKind::Power { volts: 12.0 });
    assert!(validate(x, &reg).is_empty(), "{:#?}", validate(x, &reg));
    assert!(erc(x, &reg, ErcContext::UserEdit, None).is_empty());
    // The same deck apart from the hierarchy level: the stage's inside is now at top level.
    let n = compile(x, &reg, &CompileOpts::default()).unwrap();
    assert!(
        n.text.contains("XX1 n_in xu1_n_m 0 lib_rc_lp") && n.text.contains("XU1 xu1_n_m out vcc vee lib_buf"),
        "{}",
        n.text
    );
    let (back, _) = apply_ops(x, &reg, &a.inverse, Author::User, None).unwrap();
    assert!(same_ir(&back, &c));
    // Only library components expand.
    let e = try_op(&c, &reg, "user", &op("instance.expand", json!({"refdes": "R9"}))).unwrap_err();
    assert_eq!(e.code, ErrorCode::ComponentNotFound);
}

#[test]
fn block_spec_can_be_retuned() {
    let reg = registry();
    let c = with_stage();
    let spec = json!({"amplitude_v": {"target": 2.0, "tol_pct": 5.0}});
    let a = try_op(&c, &reg, "user", &op("block.set_spec", json!({"id": "b2", "spec": spec}))).unwrap();
    assert_eq!(a.circuit.blocks["b2"].spec["amplitude_v"].target, 2.0);
    let (back, _) = apply_ops(&a.circuit, &reg, &a.inverse, Author::User, None).unwrap();
    assert!(same_ir(&back, &c));
}

#[test]
fn test_plans_and_requirements() {
    let reg = registry();
    let c = with_stage();
    let v = c.parts.keys().find(|r| r.starts_with('V') && c.parts[*r].part == "vsource_sine").unwrap().clone();
    let tests = json!([{"id": "freq_compare", "title": "Three frequencies at one amplitude",
        "scenarios": [{"name": "100 Hz", "set": {format!("{v}.frequency"): "100"}},
                      {"name": "1 kHz", "set": {format!("{v}.frequency"): "1k"}},
                      {"name": "sweep", "analyses": [{"type": "ac", "points_per_decade": 20, "f_start": 10.0, "f_stop": 20000.0}]}],
        "probes": [{"name": "out", "net": "OUT", "measure": {"kind": "amplitude"}},
                   {"name": "gain", "net": "OUT", "measure": {"kind": "ac_gain", "in": source_net(&c), "at_hz": 1000.0}}]}]);
    let a = try_op(&c, &reg, "user", &op("test.set", json!({"tests": tests}))).unwrap();
    assert!(a.patch.tests_changed);
    let reqs = json!([
        {"id": "R1", "text": "Low-pass near 200 Hz", "kind": "electrical",
         "evidence": [{"kind": "probe", "test": "freq_compare", "scenario": "100 Hz", "probe": "out", "target": 0.5, "tol_pct": 10.0}]},
        {"id": "R2", "text": "Op-amps are LM358", "kind": "constraint", "evidence": [{"kind": "structural", "claim": "every op-amp is an LM358"}]}
    ]);
    let b = try_op(&a.circuit, &reg, "user", &op("req.set", json!({"requirements": reqs}))).unwrap();
    assert!(validate(&b.circuit, &reg).is_empty());

    let mut bad = tests.clone();
    bad[0]["scenarios"][0]["set"] = json!({format!("{v}.frequency"): "fast", "R77.resistance": "1k", "nodot": "1"});
    bad[0]["probes"][0]["net"] = json!("N_NOPE");
    let e = try_op(&c, &reg, "user", &op("test.set", json!({"tests": bad}))).unwrap_err();
    assert_eq!(e.code, ErrorCode::TestInvalid);
    for want in ["frequency:", "no part R77", "is not REFDES.param", "no net N_NOPE"] {
        assert!(e.message.contains(want), "missing {want}: {}", e.message);
    }
    let bad = json!([{"id": "R1", "text": "x", "kind": "electrical", "evidence": [{"kind": "check", "block": "b9", "check": "fc_hz"}]},
                     {"id": "R1", "text": "", "kind": "test"}]);
    let e = try_op(&c, &reg, "user", &op("req.set", json!({"requirements": bad}))).unwrap_err();
    for want in ["no check b9.fc_hz", "ids are unique", "text must be"] {
        assert!(e.message.contains(want), "missing {want}: {}", e.message);
    }
    // A plan naming a part that goes later is reported when it runs, not when a snapshot loads.
    let later = run(&b.circuit, &reg, "user", &[op("part.remove", json!({"refdes": "R9"}))]);
    assert!(validate(&later, &reg).is_empty());
}

#[test]
fn version_one_snapshots_migrate() {
    let reg = Arc::new(registry());
    let c = with_stage();
    let s = Session::new(reg.clone(), Some(c.clone())).unwrap();
    assert_eq!(s.circuit(), &c);
    let mut v1 = serde_json::to_value(run(&Circuit::new(reg.version.clone()), &reg, "llm", &supply_ops())).unwrap();
    v1["schema_version"] = json!(1);
    let old: Circuit = serde_json::from_value(v1).unwrap();
    let s = Session::new(reg.clone(), Some(old)).unwrap();
    assert_eq!(s.circuit().schema_version, 2);
    let mut v9 = serde_json::to_value(&c).unwrap();
    v9["schema_version"] = json!(9);
    assert!(Session::new(reg, Some(serde_json::from_value(v9).unwrap())).is_err(), "a newer schema is refused");
    // A v2 circuit without a library serialises without the new fields.
    let plain = serde_json::to_value(Circuit::new("x")).unwrap();
    assert!(plain.get("library").is_none() && plain.get("tests").is_none());
}

#[test]
fn library_ops_are_in_the_inverse_of_a_removal() {
    // Removing a part restores it with ops that name its library definition, which is still there.
    let reg = registry();
    let c = with_stage();
    let a = try_op(&c, &reg, "user", &op("part.remove", json!({"refdes": "XU1"}))).unwrap();
    assert!(matches!(a.inverse[0], Op::PartAdd(_)));
    let (back, _) = apply_ops(&a.circuit, &reg, &a.inverse, Author::User, None).unwrap();
    assert!(same_ir(&back, &c));
}
