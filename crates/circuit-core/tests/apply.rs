mod common;

use circuit_core::ir::{BlockStatus, NetKind, Origin, PinRef};
use circuit_core::ops::{Author, Op};
use circuit_core::{Circuit, ErrorCode, apply, apply_all, apply_ops, validate};
use common::*;
use serde_json::{Value, json};

fn expect_code(c: &Circuit, author: &str, o: Value, code: ErrorCode) {
    let reg = registry();
    match try_op(c, &reg, author, &o) {
        Ok(_) => panic!("{o} should fail with {code:?}"),
        Err(e) => assert_eq!(e.code, code, "{o}: {}", e.message),
    }
}

fn base() -> Circuit {
    let reg = registry();
    let c = run(&Circuit::new(reg.version.clone()), &reg, "llm", &supply_ops());
    run(&c, &reg, "llm", &source_ops())
}

#[test]
fn demo_circuit_builds_and_validates() {
    let reg = registry();
    let c = demo_circuit(&reg);
    assert!(validate(&c, &reg).is_empty(), "{:#?}", validate(&c, &reg));
    let ops = supply_ops().len() + source_ops().len() + sallen_key_ops().len() + 1;
    assert_eq!(c.rev, ops as u64);
    assert_eq!(c.parts.len(), 8);
    assert_eq!(c.blocks["b3"].status, BlockStatus::Committed);
    assert_eq!(c.parts["R1"].block.as_deref(), Some("b3"), "envelope block is inherited");
    assert_eq!(c.parts["R1"].origin, Origin::Llm { job_id: "j_test".into() });
    assert_eq!(c.parts["C1"].params["capacitance"].display, "22nF");
    assert_eq!(c.nets["GND"].kind, NetKind::Ground);
    assert_eq!(c.nets["VEE"].kind, NetKind::Power { volts: -12.0 });
}

#[test]
fn params_take_defaults_and_parse_units() {
    let reg = registry();
    let c = run(&base(), &reg, "user", &[op("part.add", json!({"refdes": "R7", "part": "resistor_th"}))]);
    assert_eq!(c.parts["R7"].params["resistance"].si, 10e3);
    assert_eq!(c.parts["R7"].origin, Origin::User);
    let c =
        run(&c, &reg, "user", &[op("part.set_param", json!({"refdes": "R7", "key": "resistance", "value": "4k7"}))]);
    assert_eq!(c.parts["R7"].params["resistance"].display, "4.7kΩ");
}

#[test]
fn every_error_code() {
    use ErrorCode::*;
    let c = base();
    let reg = registry();
    let add = |r: &str, p: &str| op("part.add", json!({"refdes": r, "part": p}));

    expect_code(&c, "user", add("R9", "resistor_smd"), PartNotInRegistry);
    expect_code(&c, "user", add("X1", "resistor_th"), RefdesInvalid);
    expect_code(&c, "user", add("R01", "resistor_th"), RefdesInvalid);
    expect_code(&c, "user", add("C1", "resistor_th"), RefdesInvalid);
    expect_code(&c, "user", add("V1", "vsource_dc"), RefdesConflict);
    expect_code(
        &c,
        "user",
        op("part.add", json!({"refdes": "R9", "part": "resistor_th", "params": {"capacitance": "1u"}})),
        ParamUnknown,
    );
    expect_code(
        &c,
        "user",
        op("part.add", json!({"refdes": "R9", "part": "resistor_th", "params": {"resistance": "abc"}})),
        BadValue,
    );
    expect_code(
        &c,
        "user",
        op("part.add", json!({"refdes": "R9", "part": "resistor_th", "params": {"resistance": "10uF"}})),
        BadValue,
    );
    expect_code(
        &c,
        "user",
        op("part.add", json!({"refdes": "R9", "part": "resistor_th", "params": {"resistance": "0.01"}})),
        ParamOutOfRange,
    );
    expect_code(&c, "llm", op("part.add", json!({"refdes": "R9", "part": "resistor_th", "block": "b1"})), BlockNotOpen);
    expect_code(
        &c,
        "user",
        op("part.add", json!({"refdes": "R9", "part": "resistor_th", "block": "b9"})),
        BlockNotFound,
    );

    let c2 = run(&c, &reg, "user", &[add("R9", "resistor_th")]);
    expect_code(&c2, "user", op("net.connect", json!({"net": "N_X", "pins": ["R9.3"]})), PinNotFound);
    expect_code(&c2, "user", op("net.connect", json!({"net": "N_X", "pins": ["R8.1"]})), PinNotFound);
    expect_code(&c2, "user", op("net.connect", json!({"net": "N_X", "pins": ["V1.P"]})), PinAlreadyConnected);
    expect_code(&c2, "user", op("net.connect", json!({"net": "N_X", "pins": ["R9.1", "R9.1"]})), PinAlreadyConnected);
    expect_code(&c2, "user", op("net.connect", json!({"net": "gnd", "pins": ["R9.1"]})), NetInvalid);
    expect_code(&c2, "user", op("net.connect", json!({"net": "nc_R9_1", "pins": ["R9.1"]})), NetInvalid);
    expect_code(
        &c2,
        "user",
        op("net.connect", json!({"net": "N_X", "pins": ["R9.1"], "kind": {"kind": "ground"}})),
        NetInvalid,
    );
    expect_code(
        &c2,
        "user",
        op("net.connect", json!({"net": "GND", "pins": ["R9.1"], "kind": {"kind": "signal"}})),
        NetInvalid,
    );
    expect_code(
        &c2,
        "user",
        op("net.connect", json!({"net": "VCC", "pins": ["R9.1"], "kind": {"kind": "power", "volts": 5.0}})),
        NetInvalid,
    );
    expect_code(&c2, "user", op("net.connect", json!({"net": "n_in", "pins": ["R9.1"]})), NetConflict);
    expect_code(&c2, "user", op("net.disconnect", json!({"net": "N_NONE", "pins": ["R9.1"]})), NetNotFound);
    expect_code(&c2, "user", op("net.disconnect", json!({"net": "VCC", "pins": ["R9.1"]})), PinNotFound);
    expect_code(&c2, "user", op("net.rename", json!({"from": "VCC", "to": "N_IN"})), NetConflict);
    expect_code(&c2, "user", op("net.rename", json!({"from": "GND", "to": "ZERO"})), NetInvalid);
    expect_code(&c2, "user", op("part.swap", json!({"refdes": "R9", "part": "cap_film"})), CategoryMismatch);
    expect_code(&c2, "user", op("part.remove", json!({"refdes": "R8"})), PartNotFound);
    expect_code(&c2, "llm", op("part.pin", json!({"refdes": "R9", "placement": {"x": 0.0, "y": 0.0}})), Forbidden);
    expect_code(
        &c2,
        "user",
        op("part.pin", json!({"refdes": "R9", "placement": {"x": 0.0, "y": 0.0, "rot": 45}})),
        BadValue,
    );
    expect_code(
        &c2,
        "user",
        op(
            "analysis.set",
            json!({"analyses": [{"type": "dc", "source": "R9", "start": 0.0, "stop": 1.0, "step": 0.1}]}),
        ),
        AnalysisInvalid,
    );
    expect_code(
        &c2,
        "user",
        op(
            "analysis.set",
            json!({"analyses": [{"type": "dc", "source": "V1", "start": 0.0, "stop": 1.0, "step": -0.1}]}),
        ),
        AnalysisInvalid,
    );
    expect_code(
        &c2,
        "user",
        op(
            "analysis.set",
            json!({"analyses": [{"type": "ac", "points_per_decade": 10, "f_start": 0.0, "f_stop": 1.0}]}),
        ),
        AnalysisInvalid,
    );
    expect_code(
        &c2,
        "user",
        op("analysis.set", json!({"analyses": [{"type": "tran", "t_step": 1.0, "t_stop": 0.5}]})),
        AnalysisInvalid,
    );
    expect_code(&c2, "user", op("block.begin", json!({"id": "b1", "role": "other", "title": "dup"})), BlockConflict);
    expect_code(&c2, "user", op("block.begin", json!({"id": "B9", "role": "other", "title": "caps"})), SchemaError);
    expect_code(&c2, "user", op("block.commit", json!({"id": "b1"})), BlockNotOpen);
    expect_code(&c2, "user", op("block.remove", json!({"id": "b1"})), BlockNotEmpty);
    expect_code(&c2, "user", op("hint.remove", json!({"hint": {"kind": "flow", "dir": "right"}})), HintNotFound);
    expect_code(&c2, "user", op("hint.add", json!({"hint": {"kind": "near", "a": "R9", "b": "R8"}})), PartNotFound);

    // Envelope-level checks.
    let mut e = env(&c2, "user", &add("R10", "resistor_th"));
    e.base_rev -= 1;
    assert_eq!(apply(&c2, &reg, &e).unwrap_err().code, StaleRev);
    let mut e = env(&c2, "user", &add("R10", "resistor_th"));
    e.v = 2;
    assert_eq!(apply(&c2, &reg, &e).unwrap_err().code, UnsupportedVersion);
    let mut other = c2.clone();
    other.registry_version = format!("{}-other", reg.version); // any version but the loaded one
    let e = env(&other, "user", &add("R10", "resistor_th"));
    assert_eq!(apply(&other, &reg, &e).unwrap_err().code, RegistryMismatch);
}

#[test]
fn limits_are_enforced() {
    let reg = registry();
    let mut c = Circuit::new(reg.version.clone());
    let ops: Vec<Value> =
        (1..=300).map(|i| op("part.add", json!({"refdes": format!("R{i}"), "part": "resistor_th"}))).collect();
    let (next, _) =
        apply_ops(&c, &reg, &ops.iter().map(|o| env(&c, "user", o).op).collect::<Vec<_>>(), Author::User, None)
            .unwrap();
    c = next;
    expect_code(&c, "user", op("part.add", json!({"refdes": "R301", "part": "resistor_th"})), ErrorCode::LimitExceeded);
}

#[test]
fn undo_of_a_streamed_block_restores_the_circuit() {
    let reg = registry();
    let before = base();
    let mut c = before.clone();
    let mut undo: Vec<Vec<Op>> = Vec::new();
    for o in sallen_key_ops() {
        let a = try_op(&c, &reg, "llm", &o).unwrap();
        undo.push(a.inverse);
        c = a.circuit;
    }
    // One undo step for the whole block transaction (LLD §4).
    let step: Vec<Op> = undo.into_iter().rev().flatten().collect();
    let (back, redo) = apply_ops(&c, &reg, &step, Author::User, None).unwrap();
    assert!(same_ir(&back, &before));
    let (again, _) = apply_ops(&back, &reg, &redo, Author::User, None).unwrap();
    assert!(same_ir(&again, &c));
}

#[test]
fn abort_discards_the_block_and_can_be_undone() {
    let reg = registry();
    let open: Vec<Value> = sallen_key_ops().into_iter().filter(|o| o["op"] != "block.commit").collect();
    let c = run_in_block(&base(), &reg, "b3", &open);
    let a = try_op(&c, &reg, "llm", &op("block.abort", json!({"id": "b3", "reason": "test"}))).unwrap();
    let after = &a.circuit;
    assert!(!after.blocks.contains_key("b3"));
    assert!(after.parts.keys().all(|r| ["V1", "V2", "V3"].contains(&r.as_str())));
    assert!(!after.nets.contains_key("N_A") && !after.nets.contains_key("N_OUT"));
    assert_eq!(after.nets["N_IN"].pins.len(), 1, "the source keeps its pin");
    assert!(after.hints.is_empty());
    let (back, _) = apply_ops(after, &reg, &a.inverse, Author::User, None).unwrap();
    assert!(same_ir(&back, &c));
}

#[test]
fn apply_all_collects_every_error_and_applies_the_rest() {
    let reg = registry();
    let c = base();
    let batch = [
        op("part.add", json!({"refdes": "R1", "part": "resistor_th"})),
        op("part.add", json!({"refdes": "R2", "part": "nope"})),
        op("net.connect", json!({"net": "N_IN", "pins": ["R1.1"]})),
        op("net.connect", json!({"net": "N_Q", "pins": ["R2.1"]})),
    ];
    let envs: Vec<_> = batch.iter().map(|o| env(&c, "llm", o)).collect(); // all at the same base_rev
    let t = apply_all(&c, &reg, &envs);
    let codes: Vec<_> = t.errors.iter().map(|e| (e.op_index.unwrap(), e.code)).collect();
    assert_eq!(codes, [(1, ErrorCode::PartNotInRegistry), (3, ErrorCode::PinNotFound)]);
    assert!(t.circuit.nets["N_IN"].pins.contains(&PinRef::new("R1", "1")));
    let (back, _) = apply_ops(&t.circuit, &reg, &t.inverse, Author::User, None).unwrap();
    assert!(same_ir(&back, &c));
}

#[test]
fn net_rename_moves_ports_and_keeps_position() {
    let reg = registry();
    let c = demo_circuit(&reg);
    let a = try_op(&c, &reg, "user", &op("net.rename", json!({"from": "N_OUT", "to": "N_VOUT"}))).unwrap();
    let pos = |c: &Circuit, id: &str| c.nets.get_index_of(id).unwrap();
    assert_eq!(pos(&a.circuit, "N_VOUT"), pos(&c, "N_OUT"));
    assert!(a.circuit.blocks["b3"].ports.iter().any(|p| p.net == "N_VOUT"));
    assert_eq!(a.patch.nets_removed, ["N_OUT"]);
    let (back, _) = apply_ops(&a.circuit, &reg, &a.inverse, Author::User, None).unwrap();
    assert!(same_ir(&back, &c));
}

#[test]
fn part_swap_remaps_by_pin_name() {
    let reg = registry();
    let c = run(
        &base(),
        &reg,
        "user",
        &[
            op("part.add", json!({"refdes": "D1", "part": "diode_1n4148"})),
            op("part.add", json!({"refdes": "C9", "part": "cap_film", "params": {"capacitance": "1u"}})),
            op("net.connect", json!({"net": "N_IN", "pins": ["D1.A", "C9.1"]})),
        ],
    );
    let a = try_op(&c, &reg, "user", &op("part.swap", json!({"refdes": "D1", "part": "led_red"}))).unwrap();
    assert_eq!(a.circuit.parts["D1"].part, "led_red");
    // cap_elec has pins P/N, not 1/2, and C9.1 is connected.
    expect_code(&c, "user", op("part.swap", json!({"refdes": "C9", "part": "cap_elec"})), ErrorCode::PinNotFound);
    let (back, _) = apply_ops(&a.circuit, &reg, &a.inverse, Author::User, None).unwrap();
    assert!(same_ir(&back, &c));
}

#[test]
fn user_may_edit_committed_blocks_and_undo_part_removal() {
    let reg = registry();
    let c = demo_circuit(&reg);
    let c2 =
        run(&c, &reg, "user", &[op("part.set_param", json!({"refdes": "R1", "key": "resistance", "value": "4.7k"}))]);
    assert_eq!(c2.parts["R1"].params["resistance"].si, 4.7e3);
    let a = try_op(&c2, &reg, "user", &op("part.remove", json!({"refdes": "U1"}))).unwrap();
    assert!(!a.circuit.parts.contains_key("U1"));
    assert!(!a.circuit.nets["N_B"].pins.iter().any(|p| p.refdes == "U1"));
    assert_eq!(a.patch.parts_removed, ["U1"]);
    let (back, _) = apply_ops(&a.circuit, &reg, &a.inverse, Author::User, None).unwrap();
    assert!(same_ir(&back, &c2));
    assert_eq!(back.parts["U1"].origin, Origin::Llm { job_id: "j_test".into() }, "undo keeps the origin");
}

#[test]
fn narration_does_not_touch_the_ir() {
    let reg = registry();
    let c = base();
    let a =
        try_op(&c, &reg, "llm", &op("narrate", json!({"refs": ["V1"], "text": "This is the supply.", "block": "b1"})))
            .unwrap();
    assert_eq!(a.circuit, c);
    assert!(a.inverse.is_empty());
    assert_eq!(a.patch, Default::default());
}

#[test]
fn hints_are_sorted_deduped_and_follow_their_parts() {
    let reg = registry();
    let c = demo_circuit(&reg);
    let ops = [
        op("hint.add", json!({"hint": {"kind": "near", "a": "R1", "b": "R2"}})),
        op("hint.add", json!({"hint": {"kind": "flow", "dir": "right"}})),
        op("hint.add", json!({"hint": {"kind": "flow", "dir": "right"}})),
    ];
    let c = run(&c, &reg, "llm", &ops);
    assert_eq!(c.hints.len(), 3);
    assert!(c.hints.is_sorted());
    let c = run(&c, &reg, "user", &[op("part.remove", json!({"refdes": "R2"}))]);
    assert_eq!(c.hints.len(), 2, "near hint left with R2");
}

#[test]
fn patch_lists_only_what_changed() {
    let reg = registry();
    let c = base();
    let a = try_op(&c, &reg, "user", &op("part.add", json!({"refdes": "R5", "part": "resistor_th"}))).unwrap();
    assert_eq!(a.patch.parts_upserted, ["R5"]);
    assert!(a.patch.nets_upserted.is_empty());
    let b = try_op(&a.circuit, &reg, "user", &op("net.connect", json!({"net": "N_IN", "pins": ["R5.1"]}))).unwrap();
    assert!(b.patch.parts_upserted.is_empty());
    assert_eq!(b.patch.nets_upserted, ["N_IN"]);
}
