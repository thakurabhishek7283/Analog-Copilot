// Binding-level tests for the Node build of @tutor/core. Core semantics are covered by the Rust
// tests and the parity gate; these cover the JS surface (constructors, throws, getters, fork).
// usage: node --test crates/circuit-core-wasm/tests/core.test.mjs   (after wasm-pack build --target nodejs --out-dir pkg-node --out-name core)
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { createRequire } from "node:module";
import { test } from "node:test";

const require = createRequire(import.meta.url);
const core = require("../pkg-node/core.js");
const manifest = readFileSync(new URL("../../../registry/manifest.yaml", import.meta.url), "utf8");
const version = manifest.match(/^version:\s*"?([^"\s]+)"?/m)[1];
const bundle = readFileSync(new URL(`../../../target/registry/registry-${version}.json`, import.meta.url), "utf8");
const registry = core.CoreRegistry.fromJson(bundle);
const env = (s, op, body, author = "user") =>
  JSON.stringify({ v: 1, seq: s.rev + 1, op, author, base_rev: s.rev, body });

test("registry loads; bad input throws", () => {
  assert.equal(registry.version, version);
  assert.ok(JSON.parse(bundle).symbols.opamp.pins.OUT);
  assert.throws(() => core.CoreRegistry.fromJson("{}"));
  assert.throws(() => new core.CoreSession(registry, '{"schema_version":1}'));
});

test("apply returns ok/err outcomes and undo restores", () => {
  const s = new core.CoreSession(registry);
  const ok = JSON.parse(s.apply(env(s, "part.add", { refdes: "R1", part: "resistor_th" }))).ok;
  assert.deepEqual(ok.patch.parts_upserted, ["R1"]);
  const data = JSON.parse(s.changes(JSON.stringify(ok.patch))).ok;
  assert.equal(data.parts.R1.params.resistance.display, "10kΩ");
  assert.equal(s.rev, 1);
  const err = JSON.parse(s.apply(env(s, "part.add", { refdes: "R1", part: "resistor_th" }))).err;
  assert.equal(err.code, "refdes_conflict");
  const undo = JSON.parse(s.applyOps(JSON.stringify(ok.inverse), "user")).ok;
  assert.deepEqual(undo.patch.parts_removed, ["R1"]);
});

test("fork is independent and snapshots restore", () => {
  const s = new core.CoreSession(registry);
  JSON.parse(s.apply(env(s, "part.add", { refdes: "C1", part: "cap_film", params: { capacitance: "4u7" } }))).ok;
  const f = s.fork();
  f.apply(env(f, "part.remove", { refdes: "C1" }));
  assert.equal(s.rev, 1);
  assert.equal(f.rev, 2);
  const r = new core.CoreSession(registry, s.snapshot());
  assert.equal(r.snapshot(), s.snapshot());
  assert.match(r.circuitText(), /C1 cap_film capacitance=4\.7µF/);
});

test("parseQuantity and coreVersion", () => {
  assert.equal(JSON.parse(core.parseQuantity("4k7", "ohm")).ok.display, "4.7kΩ");
  assert.ok(JSON.parse(core.parseQuantity("4k7", "parsec")).err);
  assert.match(core.coreVersion(), /^\d+\.\d+\.\d+$/);
});

test("edit helpers: nextRefdes and connect", () => {
  const s = new core.CoreSession(registry);
  assert.equal(JSON.parse(s.nextRefdes("cap_film")).ok, "C1");
  for (const refdes of ["C1", "C2"]) s.apply(env(s, "part.add", { refdes, part: "cap_film" }));
  const ops = JSON.parse(s.connect("C1.1", JSON.stringify({ pin: "C2.2" }))).ok;
  assert.deepEqual(ops, [{ op: "net.connect", body: { net: "N1", pins: ["C1.1", "C2.2"] } }]);
  JSON.parse(s.applyOps(JSON.stringify(ops), "user")).ok;
  assert.equal(JSON.parse(s.connect("C2.2", JSON.stringify({ net: "N1" }))).err.code, "pin_already_connected");
  assert.equal(JSON.parse(s.connect("C2", "{}")).err.code, "schema_error");
});

test("block templates: preview, insert, verify points, spec checks", () => {
  const s = new core.CoreSession(registry);
  const req = JSON.stringify({ template: "sallen_key_lp", targets: { fc_hz: "2k" } });
  const preview = JSON.parse(s.previewBlock(req)).ok;
  assert.equal(preview.targets.fc_hz.display, "2kHz");
  assert.deepEqual(Object.keys(preview.values).sort(), ["C1", "C2", "R1", "R2", "U1"]);
  const ins = JSON.parse(s.insertBlock(req)).ok;
  assert.equal(ins.block, "b1");
  assert.equal(ins.ops[0].op, "block.begin");
  assert.equal(ins.ops.at(-1).op, "block.commit");
  JSON.parse(s.applyOps(JSON.stringify(ins.ops), "template")).ok;
  assert.equal(JSON.parse(s.snapshot()).parts.U1.origin.kind, "template");
  assert.equal(JSON.parse(s.insertBlock(JSON.stringify({ template: "nope" }))).err.code, "template_not_found");

  let netlist = JSON.parse(s.compile('{"interactive": true}')).ok;
  assert.match(netlist.checks[0].missing, /needs a signal/);
  const src = JSON.parse(s.insertBlock(JSON.stringify({ template: "sine_source", ports: { out: { net: "B1_IN" } } }))).ok;
  JSON.parse(s.applyOps(JSON.stringify(src.ops), "template")).ok;
  netlist = JSON.parse(s.compile('{"interactive": true}')).ok;
  assert.deepEqual(netlist.checks.map((c) => c.name), ["fc_hz", "q", "amplitude_v"]);
  const meas = { b1_fc_hz_ref: 0, b1_fc_hz_x: 2050 };
  const results = JSON.parse(core.evaluateChecks(JSON.stringify(netlist.checks), JSON.stringify(meas))).ok;
  assert.equal(results[0].pass, true);
  assert.equal(results[0].measured_display, "2.05kHz");
  assert.equal(results[1].pass, false);
  assert.equal(JSON.parse(registry.verifyPoints("sallen_key_lp")).ok.length, 5);
});

test("a template block's verification bench", () => {
  const s = new core.CoreSession(registry);
  const ins = JSON.parse(s.insertBlock(JSON.stringify({ template: "rc_lowpass" }))).ok;
  JSON.parse(s.applyOps(JSON.stringify(ins.ops), "template")).ok;
  const bench = JSON.parse(s.benchOps("b1")).ok;
  assert.equal(bench.length, 6);
  JSON.parse(s.applyOps(JSON.stringify(bench), "user")).ok;
  assert.equal(JSON.parse(s.benchOps("b9")).err.code, "block_not_found");
  s.free();
});

test("tutor context and answers", () => {
  const s = new core.CoreSession(registry);
  const ins = JSON.parse(s.insertBlock(JSON.stringify({ template: "rc_lowpass" }))).ok;
  JSON.parse(s.applyOps(JSON.stringify(ins.ops), "template")).ok;
  const req = { question: "Why this R1?", selection: { kind: "part", refdes: "R1" }, rev: s.rev, sim: { status: "ok", op_v: { B1_OUT: 0.5 } } };
  const ctx = JSON.parse(s.tutorContext(JSON.stringify(req))).ok;
  assert.deepEqual([ctx.parts, ctx.blocks, ctx.hazards], [["R1", "C1"], ["b1"], []]);
  assert.ok(ctx.text.includes("SELECTED: part R1") && ctx.text.includes("B1_OUT: C1.1 R1.2 | op 500mV"));
  const text = "😀 [R1] and [R9].";
  const a = JSON.parse(s.readAnswer(text));
  assert.deepEqual([a.refs_valid, a.refs_invalid], [1, 1]);
  assert.equal(a.body.slice(a.refs[0].start, a.refs[0].end), "[R1]", "spans are UTF-16 indices");
  s.free();
});

test("what changed between two sessions", () => {
  const before = new core.CoreSession(registry);
  const ins = JSON.parse(before.insertBlock(JSON.stringify({ template: "rc_lowpass" }))).ok;
  JSON.parse(before.applyOps(JSON.stringify(ins.ops), "template")).ok;
  const after = new core.CoreSession(registry, before.snapshot());
  const set = [{ op: "part.set_param", body: { refdes: "R1", key: "resistance", value: "2k" } }];
  JSON.parse(after.applyOps(JSON.stringify(set), "user")).ok;
  const req = { from_rev: before.rev, rev: after.rev, before: { op_v: { B1_OUT: 0.5 } }, after: { op_v: { B1_OUT: 0.25 } } };
  const ctx = JSON.parse(after.tutorChanges(before, JSON.stringify(req))).ok;
  assert.deepEqual([ctx.parts, ctx.nets], [["R1"], ["B1_OUT"]]);
  assert.ok(ctx.text.includes("B1_OUT: op 500mV → 250mV"), ctx.text);
  before.free();
  after.free();
});

/** A behavioral VCA with a contract (LLD §17 D3, D10). */
const vca = {
  part: {
    id: "lib:vca", category: "X", prefix: "U", title: "VCA", symbol: "auto",
    pins: [{ name: "IN", num: 1, type: "input" }, { name: "CTL", num: 2, type: "input" }, { name: "REF", num: 3, type: "passive" }, { name: "OUT", num: 4, type: "output" }],
    params: { k: { unit: "unitless", default: "2", min: 0.5, max: 4 } },
  },
  version: 1,
  model: { kind: "behavioral", laws: ["V(OUT) = V(REF) + k*(V(CTL)/1V)*(V(IN) - V(REF))", "R(IN, REF) = 1Meg", "R(CTL) = 1Meg", "require V(CTL) in [0, 5]"] },
  provenance: { kind: "user" },
  contract: {
    bench: {
      parts: { V1: { part: "vsource_dc", params: { voltage: "2.5" } }, V2: { part: "vsource_dc", params: { voltage: "1" } }, V3: { part: "vsource_sine", params: { offset: "2.5", amplitude: "0.1" } } },
      nets: { REF: ["V1.P", "DUT.REF"], CTL: ["V2.P", "DUT.CTL"], IN: ["V3.P", "DUT.IN"], OUT: ["DUT.OUT"], GND: ["V1.N", "V2.N", "V3.N"] },
    },
    checks: [{ name: "gain", label: "Gain", symbol: "G", kind: "ac_gain", in: "IN", out: "OUT", tol_pct: 1, target: "k" }],
  },
};

test("a behavioral component: specialised sub-circuits, characterisation, envelope", () => {
  const s = new core.CoreSession(registry);
  JSON.parse(s.applyOps(JSON.stringify([{ op: "lib.add", body: { def: vca } }]), "user")).ok;
  const runs = JSON.parse(s.characteriseBenches("lib:vca")).ok;
  assert.equal(runs.length, 5);
  assert.ok(runs[0].netlist.text.includes(".subckt lib_vca_"), runs[0].netlist.text);
  const outcomes = runs.map((r) => ({ meas: Object.fromEntries(r.netlist.meas.map((m) => [m.name, m.name.endsWith("_o") ? 6.02 : 0])) }));
  const rep = JSON.parse(s.characteriseReport("lib:vca", "47", JSON.stringify(runs), JSON.stringify(outcomes))).ok;
  assert.equal(rep.def_hash.length, 64);
  assert.equal(rep.points.length, 5);
  const env = JSON.parse(core.evaluateEnvelope(JSON.stringify([{ part: "U1", law: 4, text: "require V(CTL) in [0, 5]", unit: "volt", lo: 0, hi: 5, meas: ["a", "b"] }]), JSON.stringify({ a: 6, b: 1 }))).ok;
  assert.deepEqual([env[0].pass, env[0].note], [false, "from 1V to 6V"]);
  const bad = { ...vca, model: { kind: "behavioral", laws: ["V(OUT) = V(IN) + I(OUT)"] } };
  const e = JSON.parse(s.applyOps(JSON.stringify([{ op: "lib.update", body: { id: "lib:vca", def: bad } }]), "user")).err;
  assert.ok(e.message.includes("cannot add V(IN) (volts) and I(OUT) (amps)"), e.message);
  s.free();
});
