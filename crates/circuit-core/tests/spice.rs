mod common;

use circuit_core::ir::Analysis;
use circuit_core::{Circuit, CompileOpts, circuit_text, compile};
use common::*;
use serde_json::json;

#[test]
fn golden_sallen_key_netlist() {
    let reg = registry();
    let n = compile(&demo_circuit(&reg), &reg, &CompileOpts::default()).unwrap();
    insta::assert_snapshot!("sallen_key_netlist", n.text);
    assert_eq!(n.includes, ["models/tl072.lib"]);
    assert_eq!(n.node_map["GND"], "0");
    assert_eq!(n.node_map["N_OUT"], "n_out");
    assert_eq!(n.hash.len(), 64);
}

/// The demo circuit as an IR snapshot, for the simulation tests in other runtimes
/// (tools/sim, apps/web) that load it with `Session(registry, snapshot)`.
/// `UPDATE_FIXTURES=1 cargo test` rewrites it after a deliberate change.
#[test]
fn demo_fixture_is_current() {
    let reg = registry();
    let path = registry_root().join("../crates/circuit-core/tests/fixtures/demo_sallen_key.json");
    let json = serde_json::to_string_pretty(&demo_circuit(&reg)).unwrap() + "\n";
    if std::env::var_os("UPDATE_FIXTURES").is_some() {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, &json).unwrap();
    }
    let on_disk = std::fs::read_to_string(&path).unwrap_or_default().replace("\r\n", "\n");
    assert!(on_disk == json, "{} is stale: run UPDATE_FIXTURES=1 cargo test -p circuit-core", path.display());
}

#[test]
fn golden_circuit_text() {
    let reg = registry();
    insta::assert_snapshot!("sallen_key_text", circuit_text(&demo_circuit(&reg), &reg));
}

#[test]
fn netlist_does_not_depend_on_op_order() {
    let reg = registry();
    let a = demo_circuit(&reg);
    // Same circuit, built with parts and nets in a different order.
    let mut ops = sallen_key_ops();
    let commit = ops.pop().unwrap();
    let begin = ops.remove(0);
    ops.reverse();
    let (parts, nets): (Vec<_>, Vec<_>) = ops.into_iter().partition(|o| o["op"] == "part.add");
    let mut c = Circuit::new(reg.version.clone());
    c = run(&c, &reg, "llm", &source_ops());
    c = run(&c, &reg, "llm", &supply_ops());
    let mut reordered = vec![begin];
    reordered.extend(parts);
    reordered.extend(nets);
    reordered.push(commit);
    c = run_in_block(&c, &reg, "b3", &reordered);
    c = run(&c, &reg, "llm", &[op("analysis.set", serde_json::to_value(json!({"analyses": a.analyses})).unwrap())]);

    let na = compile(&a, &reg, &CompileOpts::default()).unwrap();
    let nb = compile(&c, &reg, &CompileOpts::default()).unwrap();
    assert_eq!(na.text, nb.text);
    assert_eq!(na.hash, nb.hash);
}

#[test]
fn values_change_the_hash() {
    let reg = registry();
    let a = demo_circuit(&reg);
    let b =
        run(&a, &reg, "user", &[op("part.set_param", json!({"refdes": "C2", "key": "capacitance", "value": "10n"}))]);
    let ha = compile(&a, &reg, &CompileOpts::default()).unwrap().hash;
    let hb = compile(&b, &reg, &CompileOpts::default()).unwrap().hash;
    assert_ne!(ha, hb);
}

#[test]
fn floating_nodes_get_a_shunt_only_when_asked() {
    let reg = registry();
    let c = run(
        &demo_circuit(&reg),
        &reg,
        "user",
        &[
            op("part.add", json!({"refdes": "C9", "part": "cap_film"})),
            op("net.connect", json!({"net": "N_OUT", "pins": ["C9.1"]})),
            op("net.connect", json!({"net": "N_FLOAT", "pins": ["C9.2"]})),
        ],
    );
    let plain = compile(&c, &reg, &CompileOpts::default()).unwrap();
    assert!(!plain.text.contains("Rshunt"));
    let shunted = compile(&c, &reg, &CompileOpts { shunt_floating: true, analyses: None }).unwrap();
    assert!(shunted.text.contains("Rshunt_n_float n_float 0 1e9\n"), "{}", shunted.text);
}

#[test]
fn partially_used_unit_gets_private_nc_nodes() {
    let reg = registry();
    let c = run(&demo_circuit(&reg), &reg, "user", &[op("net.connect", json!({"net": "N_IN", "pins": ["U1.INP_B"]}))]);
    let n = compile(&c, &reg, &CompileOpts { shunt_floating: true, analyses: None }).unwrap();
    assert!(n.text.contains("XU1_B n_in nc_u1_inm_b vcc vee nc_u1_out_b TL072\n"), "{}", n.text);
    assert!(n.text.contains("Rshunt_nc_u1_inm_b nc_u1_inm_b 0 1e9\n"));
}

#[test]
fn analyses_override_and_default() {
    let reg = registry();
    let mut c = demo_circuit(&reg);
    let opts =
        CompileOpts { shunt_floating: false, analyses: Some(vec![Analysis::Tran { t_step: 1e-6, t_stop: 5e-3 }]) };
    let n = compile(&c, &reg, &opts).unwrap();
    assert!(n.text.contains("\n.tran 1e-6 5e-3\n") && !n.text.contains(".ac "));
    c.analyses.clear();
    assert!(compile(&c, &reg, &CompileOpts::default()).unwrap().text.contains("\n.op\n"));
}

#[test]
fn dc_sweep_goes_and_returns_with_its_source() {
    let reg = registry();
    let sweep = json!({"analyses": [{"type": "dc", "source": "V3", "start": -1.0, "stop": 1.0, "step": 0.1}]});
    let c = run(&demo_circuit(&reg), &reg, "user", &[op("analysis.set", sweep)]);
    assert!(compile(&c, &reg, &CompileOpts::default()).unwrap().text.contains("\n.dc V3 -1e0 1e0 1e-1\n"));

    let a = try_op(&c, &reg, "user", &op("part.remove", json!({"refdes": "V3"}))).unwrap();
    assert!(a.circuit.analyses.is_empty(), "the sweep left with V3");
    let (back, _) =
        circuit_core::apply_ops(&a.circuit, &reg, &a.inverse, circuit_core::ops::Author::User, None).unwrap();
    assert_eq!(back.analyses, c.analyses, "undo brings the sweep back");

    // A hand-built (unvalidated) circuit with a dangling sweep is a compile error, not a bad netlist.
    let mut broken = a.circuit.clone();
    broken.analyses = c.analyses.clone();
    let e = compile(&broken, &reg, &CompileOpts::default()).unwrap_err();
    assert_eq!(e.refdes.as_deref(), Some("V3"));
}

/// Every registry part, every pin on its own net: compiles, and emits the expected line(s).
#[test]
fn every_part_compiles() {
    let reg = registry();
    let mut lines = Vec::new();
    for (id, def) in &reg.parts {
        let refdes = format!("{}1", def.category.letter());
        let mut ops = vec![op("part.add", json!({"refdes": refdes, "part": id}))];
        for pin in &def.pins {
            ops.push(op(
                "net.connect",
                json!({"net": format!("N_{}", pin.name), "pins": [format!("{refdes}.{}", pin.name)]}),
            ));
        }
        let c = run(&Circuit::new(reg.version.clone()), &reg, "user", &ops);
        let n = compile(&c, &reg, &CompileOpts::default()).unwrap_or_else(|e| panic!("{id}: {e}"));
        let body: Vec<&str> = n.text.lines().filter(|l| !l.starts_with('.') && !l.starts_with('*')).collect();
        assert!(!body.is_empty(), "{id} emitted nothing");
        lines.push(format!("{id}: {}", body.join(" | ")));
    }
    insta::assert_snapshot!("every_part", lines.join("\n"));
}
