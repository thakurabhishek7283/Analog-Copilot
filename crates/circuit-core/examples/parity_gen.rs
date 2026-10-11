//! Cross-runtime parity, step 1 (LLD §15): generate deterministic random op logs and the native
//! digests that the WASM (Node) and PyO3 (Python) runners must reproduce exactly.
//!
//! `cargo run -p circuit-core --example parity_gen -- <out_dir> [count] [seed]`
//! Writes `<out_dir>/logs.json` and `<out_dir>/native.json`. See tools/parity/README.md.

#[path = "support/registry_dir.rs"]
mod registry_dir;

use std::fs;
use std::path::PathBuf;
use std::sync::Arc;

use circuit_core::Registry;
use circuit_core::session::{Session, json_api as api};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

/// xorshift64*: tiny, deterministic, dependency-free.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
    fn chance(&mut self, pct: u64) -> bool {
        self.next() % 100 < pct
    }
    fn pick<'a, T>(&mut self, xs: &'a [T]) -> &'a T {
        &xs[self.below(xs.len())]
    }
}

const PARTS: &[(&str, &[&str], &[&str])] = &[
    // (part, param keys to poke, good values)
    ("resistor_th", &["resistance"], &["1k", "4k7", "10k", "220", "1meg"]),
    ("cap_film", &["capacitance"], &["100n", "22n", "1u"]),
    ("cap_elec", &["capacitance"], &["10u", "470u"]),
    ("inductor", &["inductance"], &["1m", "10u"]),
    ("diode_1n4148", &[], &[]),
    ("led_red", &[], &[]),
    ("npn_2n3904", &[], &[]),
    ("opamp_tl072", &[], &[]),
    ("opamp_lm358", &[], &[]),
    ("pnp_2n3906", &[], &[]),
    ("zener_5v1", &[], &[]),
    ("reg_lm7805", &[], &[]),
    ("timer_ne555", &[], &[]),
    ("vsource_dc", &["voltage"], &["5", "12", "-12", "3.3"]),
    ("vsource_sine", &["amplitude", "frequency", "offset"], &["1", "1k", "0", "2.5"]),
];
const BAD_VALUES: &[&str] = &["bad", "1e12", "-1", "10uF", "", "4k7k"];
const NET_NAMES: &[&str] = &["GND", "VCC", "VEE", "N_IN", "N_OUT", "N_A", "N_B", "n_a", "gnd", "nc_x"];
const BLOCKS: &[&str] = &["b1", "b2", "b3"];
/// Library components (LLD §17): a passive RC, an op-amp buffer, and a stage that nests both.
const LIB_IDS: &[&str] = &["lib:rc_lp", "lib:buf", "lib:stage"];

fn lib_def(id: &str, variant: usize) -> Value {
    let pin = |name: &str, num: u16, kind: &str| json!({"name": name, "num": num, "type": kind});
    let (title, prefix, pins, parts, nets) = match id {
        "lib:rc_lp" => (
            "RC low-pass",
            "X",
            vec![pin("IN", 1, "input"), pin("OUT", 2, "output"), pin("COM", 3, "passive")],
            json!({"R1": {"part": "resistor_th", "params": {"resistance": (["10k", "4k7", "1meg"][variant % 3])}},
                   "C1": {"part": "cap_film", "params": {"capacitance": "100n"}}}),
            json!({"IN": ["R1.1"], "OUT": ["R1.2", "C1.1"], "COM": ["C1.2"]}),
        ),
        "lib:buf" => (
            "Buffer",
            "U",
            vec![
                pin("IN", 1, "input"),
                pin("OUT", 2, "output"),
                pin("VCC", 3, "power_pos"),
                pin("VEE", 4, "power_neg"),
            ],
            json!({"U1": {"part": (["opamp_tl072", "opamp_lm358"][variant % 2])}}),
            json!({"IN": ["U1.INP_A"], "OUT": ["U1.OUT_A", "U1.INM_A"], "VCC": ["U1.VCC"], "VEE": ["U1.VEE"]}),
        ),
        _ => (
            "Filtered buffer",
            "XU",
            vec![
                pin("IN", 1, "input"),
                pin("OUT", 2, "output"),
                pin("VCC", 3, "power_pos"),
                pin("VEE", 4, "power_neg"),
            ],
            json!({"X1": {"part": "lib:rc_lp"}, "U1": {"part": "lib:buf"}}),
            json!({"IN": ["X1.IN"], "N_M": ["X1.OUT", "U1.IN"], "OUT": ["U1.OUT"], "VCC": ["U1.VCC"],
                   "VEE": ["U1.VEE"], "GND": ["X1.COM"]}),
        ),
    };
    let mut def = json!({
        "part": {"id": id, "category": "X", "prefix": prefix, "title": title, "pins": pins, "symbol": "auto"},
        "version": 1 + variant as u32,
        "model": {"kind": "composite", "parts": parts, "nets": nets},
        "provenance": {"kind": "user"},
    });
    if variant == 3 {
        def["model"]["nets"]["IN"] = json!(["R9.1"]); // names no part: refused
    }
    def
}
const ROLES: &[&str] = &["supply", "source", "amplifier", "filter"];

struct Gen<'a> {
    rng: Rng,
    reg: &'a Registry,
}

impl Gen<'_> {
    fn existing_part(&mut self, s: &Session) -> Option<String> {
        let parts: Vec<&String> = s.circuit().parts.keys().collect();
        (!parts.is_empty()).then(|| (*self.rng.pick(&parts)).clone())
    }

    fn pin_of(&mut self, s: &Session, refdes: &str) -> String {
        let part = &s.circuit().parts[refdes].part;
        let pins = &circuit_core::library::part_def(self.reg, s.circuit(), part).expect("parts have defs").pins;
        format!("{refdes}.{}", self.rng.pick(pins).name)
    }

    fn body(&mut self, s: &Session) -> (&'static str, Value) {
        let c = s.circuit();
        let r = self.rng.below(112);
        match r {
            0..=21 => {
                let (part, keys, good) = if self.rng.chance(35) {
                    (*self.rng.pick(LIB_IDS), &[][..], &[][..])
                } else {
                    *self.rng.pick(PARTS)
                };
                let prefix = match circuit_core::library::part_def(self.reg, c, part) {
                    Some(d) => d.refdes_prefix(),
                    None => "X".to_string(), // a library component not added yet: refused
                };
                let letter = if self.rng.chance(5) { "R" } else { prefix.as_str() }; // sometimes mismatched
                let mut body = json!({"refdes": format!("{letter}{}", 1 + self.rng.below(12)), "part": part});
                if !keys.is_empty() && self.rng.chance(60) {
                    let v = if self.rng.chance(15) { *self.rng.pick(BAD_VALUES) } else { *self.rng.pick(good) };
                    body["params"] = json!({ *self.rng.pick(keys): v });
                }
                if self.rng.chance(40) {
                    body["block"] = json!(self.rng.pick(BLOCKS));
                }
                ("part.add", body)
            }
            22..=46 => {
                let Some(first) = self.existing_part(s) else { return ("narrate", json!({"refs": [], "text": "x"})) };
                let mut pins = vec![self.pin_of(s, &first)];
                for _ in 0..self.rng.below(3) {
                    if let Some(p) = self.existing_part(s) {
                        pins.push(self.pin_of(s, &p));
                    }
                }
                let mut body = json!({"net": self.rng.pick(NET_NAMES), "pins": pins});
                match self.rng.below(10) {
                    0 => body["kind"] = json!({"kind": "power", "volts": 12.0}),
                    1 => body["kind"] = json!({"kind": "power", "volts": -12.0}),
                    2 => body["kind"] = json!({"kind": "ground"}),
                    _ => {}
                }
                ("net.connect", body)
            }
            47..=54 => {
                let nets: Vec<_> = c.nets.values().collect();
                if nets.is_empty() {
                    return ("narrate", json!({"refs": [], "text": "x"}));
                }
                let net = *self.rng.pick(&nets);
                let take = 1 + self.rng.below(net.pins.len());
                let pins: Vec<String> = net.pins.iter().take(take).map(|p| p.to_string()).collect();
                ("net.disconnect", json!({"net": net.id, "pins": pins}))
            }
            55..=57 => {
                let from = c.nets.keys().nth(self.rng.below(c.nets.len().max(1))).cloned().unwrap_or("N_A".into());
                ("net.rename", json!({"from": from, "to": self.rng.pick(NET_NAMES)}))
            }
            58..=63 => {
                let Some(r) = self.existing_part(s) else { return ("narrate", json!({"refs": [], "text": "x"})) };
                let part = c.parts[&r].part.clone();
                let (_, keys, good) = *PARTS.iter().find(|p| p.0 == part).unwrap_or(&("", &[], &[]));
                let key = if keys.is_empty() { "nope" } else { *self.rng.pick(keys) };
                let v = if good.is_empty() || self.rng.chance(15) {
                    *self.rng.pick(BAD_VALUES)
                } else {
                    *self.rng.pick(good)
                };
                ("part.set_param", json!({"refdes": r, "key": key, "value": v}))
            }
            64..=67 => {
                let Some(r) = self.existing_part(s) else { return ("narrate", json!({"refs": [], "text": "x"})) };
                ("part.remove", json!({"refdes": r}))
            }
            68..=69 => {
                let Some(r) = self.existing_part(s) else { return ("narrate", json!({"refs": [], "text": "x"})) };
                ("part.swap", json!({"refdes": r, "part": self.rng.pick(PARTS).0}))
            }
            70..=71 => {
                let Some(r) = self.existing_part(s) else { return ("narrate", json!({"refs": [], "text": "x"})) };
                let placement = if self.rng.chance(80) {
                    json!({"x": self.rng.below(2000) as f64 * 0.5, "y": -(self.rng.below(500) as f64), "rot": 90 * self.rng.below(4), "flip": self.rng.chance(20)})
                } else {
                    Value::Null
                };
                ("part.pin", json!({"refdes": r, "placement": placement}))
            }
            72..=77 => {
                let net = *self.rng.pick(NET_NAMES);
                (
                    "block.begin",
                    json!({"id": self.rng.pick(BLOCKS), "role": self.rng.pick(ROLES), "title": "Stage",
                    "spec": {"fc_hz": {"target": 1000.0, "tol_pct": 10.0}},
                    "ports": [{"name": "p", "direction": "input", "net": net}]}),
                )
            }
            78..=80 => ("block.commit", json!({"id": self.rng.pick(BLOCKS)})),
            81..=82 => ("block.abort", json!({"id": self.rng.pick(BLOCKS), "reason": "repair failed"})),
            83 => ("block.remove", json!({"id": self.rng.pick(BLOCKS)})),
            84..=87 => {
                let v = c.parts.keys().find(|r| r.starts_with('V')).cloned().unwrap_or("V1".into());
                let a = match self.rng.below(4) {
                    0 => json!([{"type": "op"}]),
                    1 => json!([{"type": "dc", "source": v, "start": -5.0, "stop": 5.0, "step": 0.25}]),
                    2 => {
                        json!([{"type": "op"}, {"type": "ac", "points_per_decade": 20, "f_start": 10.0, "f_stop": 1e5}])
                    }
                    _ => json!([{"type": "tran", "t_step": 1e-6, "t_stop": 2e-3}]),
                };
                ("analysis.set", json!({"analyses": a}))
            }
            88..=91 => {
                let a = self.existing_part(s).unwrap_or("R1".into());
                let b = self.existing_part(s).unwrap_or("R2".into());
                let hint = match self.rng.below(3) {
                    0 => json!({"kind": "flow", "dir": "right"}),
                    1 => json!({"kind": "near", "a": a, "b": b}),
                    _ => json!({"kind": "group", "block": self.rng.pick(BLOCKS)}),
                };
                (if self.rng.chance(75) { "hint.add" } else { "hint.remove" }, json!({"hint": hint}))
            }
            92..=95 => {
                let id = *self.rng.pick(LIB_IDS);
                ("lib.add", json!({"def": lib_def(id, self.rng.below(4))}))
            }
            96..=97 => {
                let id = *self.rng.pick(LIB_IDS);
                ("lib.update", json!({"id": id, "def": lib_def(id, self.rng.below(4))}))
            }
            98 => ("lib.remove", json!({"id": self.rng.pick(LIB_IDS)})),
            99..=101 => {
                let lib_part = c.parts.values().find(|p| p.part.starts_with("lib:")).map(|p| p.refdes.clone());
                let r = lib_part.or_else(|| self.existing_part(s)).unwrap_or("X1".into());
                let mut body = json!({"refdes": r});
                if self.rng.chance(30) {
                    body["block"] = json!(self.rng.pick(BLOCKS));
                }
                ("instance.expand", body)
            }
            102..=103 => {
                let target = if self.rng.chance(10) { f64::NAN } else { 500.0 * (1 + self.rng.below(4)) as f64 };
                let spec = if target.is_nan() {
                    json!({"fc_hz": {"target": "nan", "tol_pct": 10.0}})
                } else {
                    json!({"fc_hz": {"target": target, "tol_pct": 10.0}})
                };
                ("block.set_spec", json!({"id": self.rng.pick(BLOCKS), "spec": spec}))
            }
            104..=106 => {
                let v = c.parts.keys().find(|r| r.starts_with('V')).cloned().unwrap_or("V1".into());
                let net = c.nets.keys().next().cloned().unwrap_or("N_A".into());
                let freq = *self.rng.pick(&["100", "1k", "5k", "bad"]);
                (
                    "test.set",
                    json!({"tests": [{"id": "freq_compare", "title": "Three frequencies",
                        "scenarios": [{"name": "low", "set": {format!("{v}.frequency"): freq}},
                                      {"name": "mid", "set": {format!("{v}.frequency"): "1k"}}],
                        "probes": [{"name": "out", "net": net, "measure": {"kind": "amplitude"}}]}]}),
                )
            }
            107..=108 => {
                let evidence = match self.rng.below(3) {
                    0 => json!([{"kind": "check", "block": self.rng.pick(BLOCKS), "check": "fc_hz"}]),
                    1 => json!([{"kind": "probe", "test": "freq_compare", "scenario": "low", "probe": "out",
                                 "target": 0.5, "tol_pct": 10.0}]),
                    _ => json!([{"kind": "structural", "claim": "every op-amp is an LM358"}]),
                };
                let kind = if self.rng.chance(80) { "electrical" } else { "filter_typo" };
                (
                    "req.set",
                    json!({"requirements": [{"id": "R1", "text": "Low-pass near 200 Hz", "kind": kind, "evidence": evidence}]}),
                )
            }
            _ => ("narrate", json!({"refs": ["R1"], "text": "Now the feedback network — 10 kΩ sets the gain."})),
        }
    }

    /// One envelope as text. A few are deliberately broken in ways only the parser sees.
    fn envelope(&mut self, s: &Session, seq: u64) -> String {
        let (op, body) = self.body(s);
        let mut env = json!({"v": 1, "seq": seq, "op": op, "author": if self.rng.chance(50) { "llm" } else { "user" },
                             "base_rev": s.circuit().rev, "body": body});
        if env["author"] == "llm" {
            env["job"] = json!("j_parity");
            if self.rng.chance(30) {
                env["block"] = json!(self.rng.pick(BLOCKS));
            }
        }
        match self.rng.below(100) {
            0 => env["base_rev"] = json!(s.circuit().rev + 1),
            1 => env["op"] = json!("part.explode"),
            2 => env["v"] = json!(7),
            3 => env["body"]["unexpected"] = json!(true),
            4 => return "{\"v\":1,\"seq\":".to_string(), // truncated JSON
            _ => {}
        }
        env.to_string()
    }
}

/// The per-log digest every runtime computes identically (mirrored in tools/parity/run_*.{mjs,py}).
fn digest(reg: &Arc<Registry>, log: &[String]) -> (String, u64, usize) {
    let mut s = api::new_session(reg.clone(), None).unwrap();
    let mut out = Vec::new();
    let mut ok = 0;
    for env in log {
        let r = api::apply(&mut s, env);
        ok += r.starts_with("{\"ok\"") as usize;
        out.push(r);
    }
    out.push(api::snapshot(&s));
    out.push(api::compile(&s, "{}"));
    out.push(api::compile(&s, r#"{"shunt_floating":true}"#));
    out.push(api::erc(&s, "user_edit", None));
    out.push(api::erc(&s, "llm_block", Some("b1")));
    out.push(s.circuit_text());
    let fresh = api::new_session(reg.clone(), None).unwrap();
    out.push(api::apply_all(
        &fresh,
        &format!("[{}]", log.iter().filter(|e| e.ends_with('}')).cloned().collect::<Vec<_>>().join(",")),
    ));
    let hash = Sha256::digest(out.join("\n").as_bytes()).iter().map(|b| format!("{b:02x}")).collect();
    (hash, s.circuit().rev, ok)
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let out_dir = PathBuf::from(args.get(1).expect("usage: parity_gen <out_dir> [count] [seed]"));
    let count: usize = args.get(2).map(|s| s.parse().unwrap()).unwrap_or(1000);
    let seed: u64 = args.get(3).map(|s| s.parse().unwrap()).unwrap_or(0x5EED_CAFE);

    let reg = Arc::new(registry_dir::load(&registry_dir::repo_root().join("registry")));
    let mut g = Gen { rng: Rng(seed | 1), reg: &reg };
    let mut logs = Vec::with_capacity(count);
    for _ in 0..count {
        let n = 5 + g.rng.below(70);
        let mut s = api::new_session(reg.clone(), None).unwrap();
        let mut log = Vec::with_capacity(n);
        for seq in 1..=n as u64 {
            let env = g.envelope(&s, seq);
            api::apply(&mut s, &env); // advance state so later base_revs and picks are realistic
            log.push(env);
        }
        logs.push(log);
    }

    let results: Vec<(String, u64, usize)> = logs.iter().map(|l| digest(&reg, l)).collect();
    let total_ops: usize = logs.iter().map(Vec::len).sum();
    let ok_ops: usize = results.iter().map(|r| r.2).sum();
    fs::create_dir_all(&out_dir).unwrap();
    fs::write(
        out_dir.join("logs.json"),
        serde_json::to_string(&json!({"registry_version": reg.version, "logs": logs})).unwrap(),
    )
    .unwrap();
    let digests: Vec<&String> = results.iter().map(|r| &r.0).collect();
    fs::write(
        out_dir.join("native.json"),
        serde_json::to_string(&json!({"runtime": "native", "digests": digests})).unwrap(),
    )
    .unwrap();
    println!(
        "{count} logs, {total_ops} ops ({ok_ops} applied), final revs up to {}",
        results.iter().map(|r| r.1).max().unwrap()
    );
}
