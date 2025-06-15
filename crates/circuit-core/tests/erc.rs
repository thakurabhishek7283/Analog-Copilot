mod common;

use circuit_core::erc::{ErcCode, Severity};
use circuit_core::{Circuit, ErcContext, erc};
use common::*;
use serde_json::{Value, json};

fn codes(c: &Circuit, ctx: ErcContext, scope: Option<&str>) -> Vec<(ErcCode, Severity)> {
    erc(c, &registry(), ctx, scope).into_iter().map(|i| (i.code, i.severity)).collect()
}

fn with(ops: &[Value]) -> Circuit {
    let reg = registry();
    run(&demo_circuit(&reg), &reg, "user", ops)
}

#[test]
fn demo_circuit_is_clean_apart_from_the_spare_opamp() {
    let reg = registry();
    let c = demo_circuit(&reg);
    assert_eq!(codes(&c, ErcContext::LlmBlock, None), [(ErcCode::UnusedUnit, Severity::Warning)]);
    assert_eq!(codes(&c, ErcContext::UserEdit, None), [(ErcCode::UnusedUnit, Severity::Info)]);
    assert!(codes(&c, ErcContext::LlmBlock, Some("b1")).is_empty(), "U1 is not in b1");
}

#[test]
fn erc001_floating_pin_and_erc002_dangling_net() {
    let c = with(&[
        op("part.add", json!({"refdes": "R9", "part": "resistor_th"})),
        op("net.connect", json!({"net": "N_X", "pins": ["R9.1"]})),
    ]);
    let issues = erc(&c, &registry(), ErcContext::LlmBlock, None);
    let floating = issues.iter().find(|i| i.code == ErcCode::FloatingPin).unwrap();
    assert_eq!(floating.rule, "ERC001");
    assert_eq!(floating.pins.iter().map(|p| p.to_string()).collect::<Vec<_>>(), ["R9.2"]);
    assert!(issues.iter().any(|i| i.code == ErcCode::DanglingNet && i.nets == ["N_X"]));
}

#[test]
fn erc003_capacitor_isolated_node_has_no_dc_path() {
    let c = with(&[
        op("part.add", json!({"refdes": "C9", "part": "cap_film"})),
        op("part.add", json!({"refdes": "R9", "part": "resistor_th"})),
        op("net.connect", json!({"net": "N_OUT", "pins": ["C9.1"]})),
        op("net.connect", json!({"net": "N_C", "pins": ["C9.2", "R9.1"]})),
        op("net.connect", json!({"net": "N_D", "pins": ["R9.2"]})),
    ]);
    let issues = erc(&c, &registry(), ErcContext::UserEdit, None);
    let i = issues.iter().find(|i| i.code == ErcCode::NoDcPath).unwrap();
    assert_eq!(i.nets, ["N_C", "N_D"], "one finding per isolated group");
    assert_eq!(i.severity, Severity::Warning);
}

#[test]
fn erc003_opamp_input_needs_a_bias_path() {
    // AC-coupled non-inverting input with no bias resistor: the classic beginner mistake.
    let c = with(&[
        op("part.add", json!({"refdes": "C9", "part": "cap_film"})),
        op("net.connect", json!({"net": "N_IN", "pins": ["C9.1"]})),
        op("net.connect", json!({"net": "N_P", "pins": ["C9.2", "U1.INP_B"]})),
        op("net.connect", json!({"net": "N_OB", "pins": ["U1.OUT_B", "U1.INM_B"]})),
    ]);
    let issues = erc(&c, &registry(), ErcContext::LlmBlock, None);
    assert!(issues.iter().any(|i| i.code == ErcCode::NoDcPath && i.nets == ["N_P"]));
    assert!(!issues.iter().any(|i| i.code == ErcCode::NoDcPath && i.nets.contains(&"N_OB".to_string())));
}

#[test]
fn erc004_and_erc007_parallel_sources() {
    let same = with(&[
        op("part.add", json!({"refdes": "V9", "part": "vsource_dc", "params": {"voltage": "12"}})),
        op("net.connect", json!({"net": "VCC", "pins": ["V9.P"]})),
        op("net.connect", json!({"net": "GND", "pins": ["V9.N"]})),
    ]);
    let c = codes(&same, ErcContext::UserEdit, None);
    assert!(c.contains(&(ErcCode::VsourceLoop, Severity::Error)), "{c:?}");
    assert!(!c.iter().any(|(k, _)| *k == ErcCode::SupplyShort));

    let different = with(&[
        op("part.add", json!({"refdes": "V9", "part": "vsource_dc", "params": {"voltage": "9"}})),
        op("net.connect", json!({"net": "VCC", "pins": ["V9.P"]})),
        op("net.connect", json!({"net": "GND", "pins": ["V9.N"]})),
    ]);
    let issues = erc(&different, &registry(), ErcContext::LlmBlock, None);
    let shorts: Vec<_> = issues.iter().filter(|i| i.code == ErcCode::SupplyShort).collect();
    assert_eq!(shorts.len(), 2, "mismatch with VCC's 12 V, and with V1: {shorts:#?}");
}

#[test]
fn erc007_shorted_source() {
    let c = with(&[
        op("part.add", json!({"refdes": "V9", "part": "vsource_dc"})),
        op("net.connect", json!({"net": "N_S", "pins": ["V9.P", "V9.N"]})),
    ]);
    let issues = erc(&c, &registry(), ErcContext::LlmBlock, None);
    assert!(issues.iter().any(|i| i.code == ErcCode::SupplyShort && i.parts == ["V9"]));
    assert!(!issues.iter().any(|i| i.code == ErcCode::VsourceLoop));
}

#[test]
fn erc004_inductor_across_supply() {
    let c = with(&[
        op("part.add", json!({"refdes": "L1", "part": "inductor"})),
        op("net.connect", json!({"net": "VCC", "pins": ["L1.1"]})),
        op("net.connect", json!({"net": "GND", "pins": ["L1.2"]})),
    ]);
    assert!(codes(&c, ErcContext::LlmBlock, None).contains(&(ErcCode::VsourceLoop, Severity::Error)));
}

#[test]
fn erc005_unpowered_and_erc008_over_voltage() {
    let reg = registry();
    let c = run(
        &Circuit::new(reg.version.clone()),
        &reg,
        "user",
        &[
            op("part.add", json!({"refdes": "U1", "part": "opamp_tl072"})),
            op("part.add", json!({"refdes": "V1", "part": "vsource_dc", "params": {"voltage": "40"}})),
            op("net.connect", json!({"net": "N_SUP", "pins": ["U1.VCC", "V1.P"]})),
            op("net.connect", json!({"net": "GND", "pins": ["U1.VEE", "V1.N"]})),
        ],
    );
    let issues = erc(&c, &reg, ErcContext::LlmBlock, None);
    let unpowered: Vec<_> = issues.iter().filter(|i| i.code == ErcCode::UnpoweredIc).collect();
    assert_eq!(unpowered.len(), 1, "VCC is on a signal net; VEE on GND is fine");
    assert!(!issues.iter().any(|i| i.code == ErcCode::OverVoltage), "unknown rail voltage: no claim");

    let c = run(
        &c,
        &reg,
        "user",
        &[
            op("net.disconnect", json!({"net": "N_SUP", "pins": ["U1.VCC", "V1.P"]})),
            op(
                "net.connect",
                json!({"net": "V40", "pins": ["U1.VCC", "V1.P"], "kind": {"kind": "power", "volts": 40.0}}),
            ),
        ],
    );
    let issues = erc(&c, &reg, ErcContext::LlmBlock, None);
    assert!(issues.iter().any(|i| i.code == ErcCode::OverVoltage && i.parts == ["U1"]));
    assert!(!issues.iter().any(|i| i.code == ErcCode::UnpoweredIc));
}

#[test]
fn erc006_two_outputs_on_one_net() {
    let c = with(&[op("net.connect", json!({"net": "N_OUT", "pins": ["U1.OUT_B"]}))]);
    let issues = erc(&c, &registry(), ErcContext::LlmBlock, None);
    let i = issues.iter().find(|i| i.code == ErcCode::OutputConflict).unwrap();
    assert_eq!(i.pins.len(), 2);
    // Unit B is now partly used, so its inputs are floating rather than an unused unit.
    assert!(issues.iter().any(|i| i.code == ErcCode::FloatingPin && i.parts == ["U1"]));
    assert!(!issues.iter().any(|i| i.code == ErcCode::UnusedUnit));
}

#[test]
fn erc009_port_bound_to_nothing() {
    let reg = registry();
    let c = run(
        &demo_circuit(&reg),
        &reg,
        "user",
        &[op(
            "block.begin",
            json!({"id": "b4", "role": "amplifier", "title": "Gain stage",
                   "ports": [{"name": "in", "direction": "input", "net": "N_OUT"}]}),
        )],
    );
    assert_eq!(codes(&c, ErcContext::LlmBlock, Some("b4")), [(ErcCode::PortUnbound, Severity::Error)]);
    assert!(!codes(&c, ErcContext::UserEdit, None).iter().any(|(k, _)| *k == ErcCode::PortUnbound));
}

#[test]
fn scope_keeps_only_the_blocks_issues() {
    let c = with(&[
        op("part.add", json!({"refdes": "R9", "part": "resistor_th"})), // floating, in no block
    ]);
    let scoped = codes(&c, ErcContext::LlmBlock, Some("b3"));
    assert_eq!(scoped, [(ErcCode::UnusedUnit, Severity::Warning)]);
    assert!(codes(&c, ErcContext::LlmBlock, None).iter().any(|(k, _)| *k == ErcCode::FloatingPin));
}

#[test]
fn erc_output_is_deterministic() {
    let reg = registry();
    let c = with(&[
        op("part.add", json!({"refdes": "R10", "part": "resistor_th"})),
        op("part.add", json!({"refdes": "R9", "part": "resistor_th"})),
    ]);
    let a = serde_json::to_string(&erc(&c, &reg, ErcContext::UserEdit, None)).unwrap();
    let b = serde_json::to_string(&erc(&c.clone(), &reg, ErcContext::UserEdit, None)).unwrap();
    assert_eq!(a, b);
    let floating: Vec<_> = erc(&c, &reg, ErcContext::UserEdit, None)
        .into_iter()
        .filter(|i| i.code == ErcCode::FloatingPin)
        .flat_map(|i| i.parts)
        .collect();
    assert_eq!(floating, ["R9", "R10"], "natural refdes order");
}

/// 12 V → LM7805 → 5 V rail → NE555 astable blinking an LED: a correct circuit is ERC-clean.
#[test]
fn regulated_555_blinker_is_clean() {
    let reg = registry();
    let ops = [
        op("part.add", json!({"refdes": "V1", "part": "vsource_dc", "params": {"voltage": "12"}})),
        op("part.add", json!({"refdes": "U1", "part": "reg_lm7805"})),
        op("part.add", json!({"refdes": "U2", "part": "timer_ne555"})),
        op("part.add", json!({"refdes": "R1", "part": "resistor_th", "params": {"resistance": "1k"}})),
        op("part.add", json!({"refdes": "R2", "part": "resistor_th", "params": {"resistance": "68k"}})),
        op("part.add", json!({"refdes": "R3", "part": "resistor_th", "params": {"resistance": "330"}})),
        op("part.add", json!({"refdes": "C1", "part": "cap_elec", "params": {"capacitance": "10u"}})),
        op("part.add", json!({"refdes": "C2", "part": "cap_film", "params": {"capacitance": "10n"}})),
        op("part.add", json!({"refdes": "D1", "part": "led_red"})),
        op("net.connect", json!({"net": "V12", "pins": ["V1.P", "U1.IN"], "kind": {"kind": "power", "volts": 12.0}})),
        op(
            "net.connect",
            json!({"net": "V5", "pins": ["U1.OUT", "U2.VCC", "U2.RESET", "R1.1"], "kind": {"kind": "power", "volts": 5.0}}),
        ),
        op("net.connect", json!({"net": "GND", "pins": ["V1.N", "U1.GND", "U2.GND", "C1.N", "C2.2", "D1.K"]})),
        op("net.connect", json!({"net": "N_DIS", "pins": ["R1.2", "R2.1", "U2.DIS"]})),
        op("net.connect", json!({"net": "N_TH", "pins": ["R2.2", "U2.THR", "U2.TRIG", "C1.P"]})),
        op("net.connect", json!({"net": "N_CTRL", "pins": ["U2.CTRL", "C2.1"]})),
        op("net.connect", json!({"net": "N_OUT", "pins": ["U2.OUT", "R3.1"]})),
        op("net.connect", json!({"net": "N_LED", "pins": ["R3.2", "D1.A"]})),
    ];
    let c = run(&Circuit::new(reg.version.clone()), &reg, "user", &ops);
    assert_eq!(erc(&c, &reg, ErcContext::LlmBlock, None), []);

    // Move the 555 onto an 18 V rail: above its 16 V rating, so ERC008 fires.
    let hot = run(
        &c,
        &reg,
        "user",
        &[
            op("net.disconnect", json!({"net": "V5", "pins": ["U2.VCC"]})),
            op("net.connect", json!({"net": "V18", "pins": ["U2.VCC"], "kind": {"kind": "power", "volts": 18.0}})),
        ],
    );
    assert!(
        erc(&hot, &reg, ErcContext::LlmBlock, None).iter().any(|i| i.code == ErcCode::OverVoltage && i.parts == ["U2"])
    );
}
