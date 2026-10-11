"""Library components on the real engines (LLD §17 D1): a circuit with nested component instances
simulates exactly like the same circuit expanded one level, and fully flat, on the native ngspice
and on the browser's ngspice.wasm. Hierarchy is only how the deck is written."""

from __future__ import annotations

import json
import shutil
import subprocess

import circuit_core as cc
import pytest

from circuits import Bench, ac, op, tran
from sim_runner.ngspice_batch import REGISTRY_DIR, REPO

REG = cc.load_registry_dir(REGISTRY_DIR)
WASM = REPO / "third_party" / "ngspice" / "dist" / "wasm" / "ngspice.mjs"
NODE = shutil.which("node")


def pin(name: str, num: int, kind: str) -> dict:
    return {"name": name, "num": num, "type": kind}


def define(b: Bench) -> Bench:
    rc = {
        "part": {"id": "lib:rc_lp", "category": "X", "title": "RC low-pass", "symbol": "auto",
                 "pins": [pin("IN", 1, "input"), pin("OUT", 2, "output"), pin("COM", 3, "passive")]},
        "version": 1,
        "model": {"kind": "composite",
                  "parts": {"R1": {"part": "resistor_th", "params": {"resistance": "1.6k"}},
                            "C1": {"part": "cap_film", "params": {"capacitance": "100n"}}},
                  "nets": {"IN": ["R1.1"], "OUT": ["R1.2", "C1.1"], "COM": ["C1.2"]}},
        "provenance": {"kind": "user"},
    }
    gain = {
        "part": {"id": "lib:gain2", "category": "X", "prefix": "U", "title": "Gain of 2", "symbol": "auto",
                 "pins": [pin("IN", 1, "input"), pin("OUT", 2, "output"), pin("VCC", 3, "power_pos"), pin("VEE", 4, "power_neg")]},
        "version": 1,
        "model": {"kind": "composite",
                  "parts": {"U1": {"part": "opamp_lm358"},
                            "R1": {"part": "resistor_th", "params": {"resistance": "10k"}},
                            "R2": {"part": "resistor_th", "params": {"resistance": "10k"}}},
                  "nets": {"IN": ["U1.INP_A"], "OUT": ["U1.OUT_A", "R1.1"], "N_M": ["U1.INM_A", "R1.2", "R2.1"],
                           "GND": ["R2.2"], "VCC": ["U1.VCC"], "VEE": ["U1.VEE"]}},
        "provenance": {"kind": "user"},
    }
    stage = {
        "part": {"id": "lib:stage", "category": "X", "prefix": "XU", "title": "Filter and gain", "symbol": "auto",
                 "pins": [pin("IN", 1, "input"), pin("OUT", 2, "output"), pin("VCC", 3, "power_pos"), pin("VEE", 4, "power_neg")]},
        "version": 1,
        "model": {"kind": "composite",
                  "parts": {"X1": {"part": "lib:rc_lp"}, "U1": {"part": "lib:gain2"}},
                  "nets": {"IN": ["X1.IN"], "N_F": ["X1.OUT", "U1.IN"], "OUT": ["U1.OUT"], "GND": ["X1.COM"],
                           "VCC": ["U1.VCC"], "VEE": ["U1.VEE"]}},
        "provenance": {"kind": "user"},
    }
    for d in (rc, gain, stage):
        b.op("lib.add", {"def": d})
    return b


def circuit() -> Bench:
    """Two stages in a row on a 12 V single supply, a 0.2 V sine on a 1.5 V bias (6 V ± 0.8 V out)."""
    b = define(Bench(REG))
    b.part("V1", "vsource_sine", offset="1.5", amplitude="0.2", frequency="500")
    b.net("N_IN", "V1.P").net("GND", "V1.N")
    for k, (src, out) in enumerate([("N_IN", "N_MID"), ("N_MID", "OUT")], start=1):
        b.part(f"XU{k}", "lib:stage")
        b.net(src, f"XU{k}.IN").net(out, f"XU{k}.OUT")
        b.net("VCC", f"XU{k}.VCC", volts=12.0).net("GND", f"XU{k}.VEE")
    b.part("R9", "resistor_th", resistance="10k").net("OUT", "R9.1").net("GND", "R9.2")
    return b.analyses(op(), tran(5e-6, 10e-3), ac(10, 100e3))


def expand_all(b: Bench) -> Bench:
    """Expand every library instance until none is left."""
    while True:
        snap = json.loads(b.session.snapshot())
        lib = [r for r, p in snap["parts"].items() if p["part"].startswith("lib:")]
        if not lib:
            return b
        b.op("instance.expand", {"refdes": lib[0]})


MEAS = (
    ".meas ac g_100 find vm(out) at=100\n"
    ".meas ac g_1k find vm(out) at=1000\n"
    ".meas ac g_10k find vm(out) at=10000\n"
    ".meas tran out_pp pp v(out) from=5m to=10m\n"
    ".meas tran out_avg avg v(out) from=5m to=10m\n"
)


def decks() -> dict[str, dict]:
    nested = circuit()
    one = circuit()
    one.op("instance.expand", {"refdes": "XU1"})
    flat = expand_all(circuit())
    out = {}
    for name, b in (("nested", nested), ("one level", one), ("flat", flat)):
        n = b.netlist()
        out[name] = {"netlist": n["text"].replace("\n.end\n", "\n" + MEAS + ".end\n"), "includes": n["includes"],
                     "hash": n["hash"], "leaf": n["leaf_devices"]}
    return out


def test_hierarchy_does_not_change_the_simulation():
    from sim_runner.ngspice_batch import simulate

    d = decks()
    assert ".subckt lib_stage" in d["nested"]["netlist"] and ".subckt" not in d["flat"]["netlist"]
    assert {v["leaf"] for v in d.values()} == {2 * 5 + 2}, "two stages of R, C, op-amp, 2 R; the source and the load"
    results = {name: simulate(v["netlist"], v["includes"], hash=v["hash"], timeout_s=10, vectors=False) for name, v in d.items()}
    for name, r in results.items():
        assert r.status == "ok", (name, r.log[-1500:])
    ref = results["flat"].meas
    # Gain 2 per stage and an RC of 995 Hz per stage: 4 × 0.995² at 100 Hz; the bias is amplified too.
    assert ref["g_100"] == pytest.approx(0.2 * 4 * 0.990, rel=0.01)
    assert ref["g_1k"] == pytest.approx(0.2 * 4 * 0.5, rel=0.03)
    assert ref["g_10k"] < 0.02
    assert ref["out_avg"] == pytest.approx(1.5 * 4, rel=0.01)
    for name in ("nested", "one level"):
        for k, v in ref.items():
            assert results[name].meas[k] == pytest.approx(v, rel=1e-9, abs=1e-12), (name, k)


@pytest.mark.skipif(not (WASM.exists() and NODE), reason="ngspice.wasm or node missing")
def test_browser_engine_agrees(tmp_path):
    from sim_runner.ngspice_batch import simulate

    d = decks()
    reqs = [{"netlist": v["netlist"], "includes": v["includes"], "hash": v["hash"]} for v in d.values()]
    (tmp_path / "requests.json").write_text(json.dumps(reqs), encoding="utf-8")
    subprocess.run([NODE, str(REPO / "tools/sim/run_wasm.mts"), str(tmp_path / "requests.json"), str(tmp_path / "results.json")],
                   check=True, cwd=REPO)
    wasm = json.loads((tmp_path / "results.json").read_text(encoding="utf-8"))
    for (name, v), w in zip(d.items(), wasm, strict=True):
        assert w["status"] == "ok", (name, w["log"][-1000:])
        native = simulate(v["netlist"], v["includes"], hash=v["hash"], timeout_s=10, vectors=False).meas
        for k, x in native.items():
            assert w["meas"][k] == pytest.approx(x, rel=1e-3), (name, k)
