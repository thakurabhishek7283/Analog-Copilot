//! Grounded tutor (LLD §9): the context a question gets, and answers read against the circuit.

mod common;

use std::collections::BTreeMap;
use std::sync::Arc;

use circuit_core::error::ErrorCode;
use circuit_core::ir::PortDirection;
use circuit_core::ops::Author;
use circuit_core::registry::Hazard;
use circuit_core::template::{InsertBlock, PortBinding};
use circuit_core::tutor::{AcValues, RefKind, Selection, SimValues, Span, TranValues};
use circuit_core::wire::AskRequest;
use circuit_core::{Registry, Session};
use common::*;

fn insert(s: &mut Session, template: &str, targets: &[(&str, &str)], ports: &[(&str, PortBinding)]) -> String {
    let req = InsertBlock {
        template: template.into(),
        targets: targets.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect(),
        ports: ports.iter().map(|(k, v)| (k.to_string(), v.clone())).collect(),
        id: None,
    };
    let ins = s.insert_block(&req).unwrap_or_else(|e| panic!("{template}: {e}"));
    s.apply_ops(&ins.ops, Author::Template).unwrap();
    ins.block
}

fn port_net(s: &Session, block: &str, dir: PortDirection) -> String {
    s.circuit().blocks[block].ports.iter().find(|p| p.direction == dir).unwrap().net.clone()
}

/// A 1 kHz sine into a 1 kHz Sallen-Key low-pass: b1 the source, b2 the filter.
fn filter(reg: Arc<Registry>) -> Session {
    let mut s = Session::new(reg, None).unwrap();
    let b1 = insert(&mut s, "sine_source", &[("freq_hz", "1k")], &[]);
    let sig = port_net(&s, &b1, PortDirection::Output);
    insert(&mut s, "sallen_key_lp", &[("fc_hz", "1k")], &[("in", PortBinding::Net(sig))]);
    s
}

fn ask(question: &str, selection: Option<Selection>, sim: SimValues) -> AskRequest {
    AskRequest {
        question: question.into(),
        selection,
        rev: 0,
        level: Default::default(),
        mode: Default::default(),
        sim,
        effort: None,
    }
}

fn part(r: &str) -> Option<Selection> {
    Some(Selection::Part { refdes: r.into() })
}

/// What the browser would send after simulating `filter`.
fn sim() -> SimValues {
    let v = |pairs: &[(&str, f64)]| pairs.iter().map(|(k, x)| (k.to_string(), *x)).collect::<BTreeMap<_, _>>();
    SimValues {
        status: Some("ok".into()),
        op_v: v(&[("B1_OUT", 0.0), ("B2_N_A", 0.0012), ("B2_OUT", 0.0024), ("VCC", 12.0)]),
        op_i: v(&[("R1.1", -6.5e-8), ("R1.2", 6.5e-8), ("U1.OUT_A", 1.2e-6)]),
        ac: Some(AcValues { hz: 100.0, v: v(&[("B1_OUT", 1.0), ("B2_N_A", 0.98)]), deg: v(&[("B2_N_A", -12.3)]) }),
        tran: Some(TranValues { t_stop: 5e-3, v: [("B2_OUT".to_string(), Span { min: -0.71, max: 0.7 })].into() }),
        checks: serde_json::from_value(serde_json::json!([{
            "block": "b2", "name": "fc_hz", "label": "Cutoff (−3 dB)", "symbol": "fc", "unit": "hertz",
            "target": 1000.0, "tol_pct": 10.0, "measured": 1003.0, "pass": true,
            "target_display": "1kHz", "measured_display": "1kHz"
        }]))
        .unwrap(),
    }
}

#[test]
fn a_selected_part_brings_its_block_its_neighbours_and_their_values() {
    let s = filter(Arc::new(registry()));
    let ctx = s.tutor_context(&ask("Why is R1 18k?", part("R1"), sim())).unwrap();
    // C2 is two hops from R1, but the cutoff R1 helps set depends on it (found in a live run).
    assert_eq!(ctx.parts, ["R1", "C1", "C2", "R2", "U1", "V1"], "R1, the rest of b2, then one hop over signal nets");
    assert_eq!(ctx.blocks, ["b1", "b2"]);
    assert!(ctx.dropped.is_empty() && ctx.hazards.is_empty());
    assert!(ctx.tokens < 500, "{}", ctx.tokens);
    insta::assert_snapshot!(ctx.text);
}

#[test]
fn nets_and_blocks_bring_no_neighbours() {
    let s = filter(Arc::new(registry()));
    let net = s.tutor_context(&ask("What is here?", Some(Selection::Net { id: "B2_N_A".into() }), sim())).unwrap();
    assert_eq!(net.parts, ["C1", "R1", "R2"], "every part on the net, nothing beyond");
    assert_eq!(net.nets[0], "B2_N_A");
    assert!(net.text.contains("B2_N_A: C1.1 R1.2 R2.1 | op 1.2mV | ac 980mV -12.3° @100Hz"), "{}", net.text);

    let block = s.tutor_context(&ask("How does it work?", Some(Selection::Block { id: "b2".into() }), sim())).unwrap();
    assert_eq!(block.parts, ["C1", "C2", "R1", "R2", "U1"]);
    assert_eq!(block.blocks, ["b2"], "the source block stays out");
    assert!(block.text.contains("spec: fc_hz 1kHz ±10% measured 1kHz pass; q 0.707 ±15%"), "{}", block.text);
}

#[test]
fn rails_are_not_walked() {
    let s = filter(Arc::new(registry()));
    // C2 sits on B2_N_B and GND; V1 is on GND too, but GND is not a path.
    let ctx = s.tutor_context(&ask("?", part("C2"), sim())).unwrap();
    assert_eq!(ctx.parts, ["C2", "C1", "R1", "R2", "U1"], "C2 and its block; V1 is not reached");
    assert!(ctx.text.contains("RAILS: GND ground; VCC power 12V | op 12V; VEE power -12V"), "{}", ctx.text);
}

#[test]
fn parts_the_question_names_are_added() {
    let s = filter(Arc::new(registry()));
    let named = s.tutor_context(&ask("What does r2 do, and why is C2 small?", None, sim())).unwrap();
    assert_eq!(&named.parts[..2], ["R2", "C2"], "named parts first, then their block and neighbours");
    assert!(named.parts.contains(&"U1".to_string()) && !named.parts.contains(&"V1".to_string()));

    let with_selection = s.tutor_context(&ask("Compare it with U1", part("R1"), sim())).unwrap();
    assert_eq!(&with_selection.parts[..2], ["R1", "U1"]);

    let everything = s.tutor_context(&ask("What is this circuit?", None, sim())).unwrap();
    assert_eq!(everything.parts, ["C1", "C2", "R1", "R2", "U1", "V1"]);
}

#[test]
fn a_big_circuit_keeps_fifteen_parts_nearest_the_selection() {
    let mut s = filter(Arc::new(registry()));
    for _ in 0..3 {
        let last = s.circuit().blocks.keys().last().unwrap().clone();
        let out = port_net(&s, &last, PortDirection::Output);
        insert(&mut s, "sallen_key_lp", &[("fc_hz", "1k")], &[("in", PortBinding::Net(out))]);
    }
    assert_eq!(s.circuit().parts.len(), 21);
    let ctx = s.tutor_context(&ask("Explain the whole thing", None, sim())).unwrap();
    assert_eq!(ctx.parts.len(), 15);
    assert_eq!(ctx.dropped.len(), 6);
    assert!(ctx.tokens as usize <= circuit_core::tutor::CONTEXT_TOKENS);

    let selected = s.tutor_context(&ask("?", Some(Selection::Block { id: "b5".into() }), sim())).unwrap();
    assert_eq!(selected.blocks, ["b5"]);
    assert_eq!(selected.parts.len(), 5);
}

#[test]
fn a_selection_the_circuit_lacks_is_refused() {
    let s = filter(Arc::new(registry()));
    let code = |sel: Selection| s.tutor_context(&ask("?", Some(sel), sim())).unwrap_err().code;
    assert_eq!(code(Selection::Part { refdes: "R9".into() }), ErrorCode::PartNotFound);
    assert_eq!(code(Selection::Net { id: "N_X".into() }), ErrorCode::NetNotFound);
    assert_eq!(code(Selection::Block { id: "b9".into() }), ErrorCode::BlockNotFound);
}

#[test]
fn mains_parts_are_flagged() {
    let mut reg = registry();
    reg.parts.get_mut("vsource_sine").unwrap().hazard = Some(Hazard::Mains);
    let s = filter(Arc::new(reg));
    let ctx = s.tutor_context(&ask("?", part("C2"), sim())).unwrap();
    assert_eq!(ctx.hazards, ["V1"], "anywhere in the circuit, not only in the slice");
    assert!(ctx.text.ends_with("SAFETY: mains-powered parts in this circuit: V1\n"), "{}", ctx.text);
}

#[test]
fn erc_findings_in_the_slice_are_listed() {
    let mut s = filter(Arc::new(registry()));
    let ops = vec![
        serde_json::from_value(op("net.disconnect", serde_json::json!({"net": "B2_N_A", "pins": ["R2.1"]}))).unwrap(),
    ];
    s.apply_ops(&ops, Author::User).unwrap();
    let ctx = s.tutor_context(&ask("Why is the output flat?", part("R2"), SimValues::default())).unwrap();
    let erc = ctx.text.split("ERC:").nth(1).unwrap();
    assert!(erc.contains("R2.1") || erc.contains("R2"), "{}", ctx.text);
    assert!(!erc.starts_with(" none"));
}

#[test]
fn answers_are_read_against_the_circuit() {
    let s = filter(Arc::new(registry()));
    let a = s.read_answer("[R1] and [C1] set fc on [net:B2_N_A] in [block:b2]; [R9] does not exist.");
    assert_eq!((a.refs_valid, a.refs_invalid), (4, 1));
    assert_eq!(a.refs.iter().find(|r| !r.valid).map(|r| (r.kind, r.id.as_str())), Some((RefKind::Part, "R9")));
    assert!(a.try_.is_none());

    let tried = |block: &str| s.read_answer(&format!("Try it.\n\n```try\n{block}\n```")).try_.unwrap();
    let ok = tried(
        r#"{"ops":[{"op":"part.set_param","body":{"refdes":"R1","key":"resistance","value":"36k"}}],"predict":"fc drops to about 700 Hz"}"#,
    );
    assert!(ok.problems.is_empty(), "{:?}", ok.problems);
    assert_eq!((ok.ops.len(), ok.predict.as_str()), (1, "fc drops to about 700 Hz"));

    let bad_value =
        tried(r#"{"ops":[{"op":"part.set_param","body":{"refdes":"R1","key":"resistance","value":"lots"}}]}"#);
    assert_eq!(bad_value.problems.len(), 1);
    let missing = tried(r#"{"ops":[{"op":"part.set_param","body":{"refdes":"R7","key":"resistance","value":"1k"}}]}"#);
    assert_eq!(missing.problems[0].code, ErrorCode::PartNotFound);
    let block_op = tried(r#"{"ops":[{"op":"block.remove","body":{"id":"b2"}}]}"#);
    assert_eq!((block_op.problems[0].code, block_op.problems[0].op_index), (ErrorCode::Forbidden, Some(0)));
    let unknown = tried(r#"{"ops":[{"op":"part.explode","body":{}}]}"#);
    assert_eq!(unknown.problems[0].code, ErrorCode::SchemaError);
    let malformed = tried("{\"ops\": [");
    assert_eq!(malformed.problems[0].code, ErrorCode::SchemaError);
    assert!(tried(r#"{"ops":[]}"#).problems[0].message.contains("no ops"));
}

#[test]
fn the_json_api_matches_the_session() {
    use circuit_core::session::json_api;
    let s = filter(Arc::new(registry()));
    let req = serde_json::json!({"question": "Why?", "selection": {"kind": "part", "refdes": "R1"}, "rev": 2});
    let out: serde_json::Value = serde_json::from_str(&json_api::tutor_context(&s, &req.to_string())).unwrap();
    assert_eq!(out["ok"]["parts"][0], "R1");
    assert!(json_api::tutor_context(&s, r#"{"question": "?"}"#).contains("schema_error"), "rev is required");
    let a: serde_json::Value = serde_json::from_str(&json_api::read_answer(&s, "See [R1].")).unwrap();
    assert_eq!(a["refs_valid"], 1);
    assert!(a.get("try").is_none());
}
