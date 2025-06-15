mod common;

use circuit_core::Registry;
use common::*;

/// CI step 1 (LLD §12): every YAML part loads, and every model file it names exists.
#[test]
fn shipped_registry_loads_and_its_files_exist() {
    let reg = registry();
    assert!(reg.parts.len() >= 10);
    for p in reg.parts.values() {
        if let Some(inc) = p.spice.as_ref().and_then(|s| s.include.as_ref()) {
            assert!(registry_root().join(inc).is_file(), "{}: missing model file {inc}", p.id);
        }
        if let Some(sym) = &p.symbol {
            assert!(registry_root().join(sym).is_file(), "{}: missing symbol {sym}", p.id);
        }
    }
}

#[test]
fn json_bundle_round_trips() {
    let reg = registry();
    let json = serde_json::to_string(&reg).unwrap();
    assert_eq!(Registry::from_json(&json).unwrap(), reg);
}

fn load_err(yaml: &str) -> String {
    let errs = Registry::from_yaml_docs("t", [("bad.yaml", yaml)]).unwrap_err();
    errs.iter().map(|e| e.to_string()).collect::<Vec<_>>().join("\n")
}

#[test]
fn rejects_broken_parts() {
    let base = |extra: &str| {
        format!(
            "id: r\ncategory: R\ntitle: t\npins:\n  - {{name: \"1\", num: 1, type: passive}}\n  - {{name: \"2\", num: 2, type: passive}}\n{extra}"
        )
    };
    assert!(load_err(&base("spice: {line: \"{refdes} {1} {3} 1k\"}")).contains("unknown placeholder {3}"));
    assert!(load_err(&base("params:\n  resistance: {unit: ohm, default: abc}")).contains("default"));
    assert!(load_err(&base("params:\n  resistance: {unit: ohm, default: 1, min: 10}")).contains("outside min/max"));
    assert!(load_err(&base("params:\n  resistance: {unit: ohm}")).contains("default"));
    assert!(load_err(&base("bogus: 1")).contains("unknown field"));
    assert!(load_err(&base("dc_param: x")).contains("only valid on voltage sources"));
    let dup = "id: r\ncategory: R\ntitle: t\npins:\n  - {name: A, num: 1, type: passive}\n  - {name: A, num: 2, type: passive}\n";
    assert!(load_err(dup).contains("duplicate pin name"));
    let unit = "id: u\ncategory: U\ntitle: t\nunits: [A]\npins:\n  - {name: OUT, num: 1, type: output, unit: A}\n";
    assert!(load_err(unit).contains("must end with _A"));
    let vsrc = "id: v\ncategory: V\ntitle: t\npins:\n  - {name: P, num: 1, type: passive}\n  - {name: M, num: 2, type: passive}\nparams:\n  voltage: {unit: volt, default: \"1\"}\ndc_param: voltage\n";
    assert!(load_err(vsrc).contains("must have pin N"));
}

#[test]
fn prefixed_limits_are_parsed() {
    let reg = registry();
    let cap = &reg.parts["cap_film"].params["capacitance"];
    assert_eq!(cap.min, Some(1e-12));
    assert_eq!(cap.max, Some(10e-6));
    assert_eq!(reg.parts["vsource_sine"].params["frequency"].max, Some(100e6));
}

/// Every SPICE template must agree with its model file: a `.model` of that name exists, or a
/// `.subckt` of that name takes exactly as many nodes as the template passes it.
#[test]
fn spice_templates_match_their_model_files() {
    let reg = registry();
    for p in reg.parts.values() {
        let Some(sp) = &p.spice else { continue };
        let Some(inc) = &sp.include else { continue };
        let lib = std::fs::read_to_string(registry_root().join(inc)).unwrap().to_ascii_lowercase();
        let line = sp.line.as_ref().or(sp.unit_line.as_ref()).unwrap();
        let tokens: Vec<&str> = line.split_whitespace().collect();
        let model = tokens.last().unwrap().to_ascii_lowercase();
        if line.starts_with('X') {
            let nodes = tokens.len() - 2;
            let header = lib
                .lines()
                .find(|l| l.split_whitespace().take(2).eq([".subckt", model.as_str()]))
                .unwrap_or_else(|| panic!("{}: no .subckt {model} in {inc}", p.id));
            let pins = header.split_whitespace().count() - 2;
            assert_eq!(nodes, pins, "{}: template passes {nodes} nodes, .subckt {model} takes {pins}", p.id);
        } else {
            assert!(
                lib.lines().any(|l| l.split_whitespace().nth(1) == Some(model.as_str()) && l.starts_with(".model")),
                "{}: no .model {model} in {inc}",
                p.id
            );
        }
    }
}
