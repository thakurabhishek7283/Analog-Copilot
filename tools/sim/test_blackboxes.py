"""The 12 reference black boxes (LLD §17, P5.2 gate): each is characterised on the native engine
and in the browser engine (ngspice.wasm through sim.engine.ts, in Node), and both give the same
report: every check passes at every point on both, and every measured value agrees to the decks'
reltol. The Linux engine runs the same characterisation inside the sim-runner image:
`python tools/sim/blackboxes.py linux` (see the module docstring)."""

from __future__ import annotations

import json
import shutil
import subprocess

import circuit_core as cc
import pytest

import blackboxes as bb
from sim_runner.ngspice_batch import REGISTRY_DIR, REPO

WASM = REPO / "third_party" / "ngspice" / "dist" / "wasm" / "ngspice.mjs"
NODE = shutil.which("node")
# Both engines solve to the decks' reltol=1e-3 (see test_wasm_parity.py).
MEAS_REL = 1e-3


@pytest.fixture(scope="module")
def prepared():
    reg = cc.load_registry_dir(REGISTRY_DIR)
    out = {}
    for name, d in bb.definitions().items():
        s, runs = bb.benches(reg, d)
        out[name] = (d, s, runs)
    return out


@pytest.fixture(scope="module")
def native(prepared):
    return {name: bb.report(s, d, "native", runs, bb.native_outcomes(runs)) for name, (d, s, runs) in prepared.items()}


def failures(rep: dict) -> list[str]:
    out = []
    for p in rep["points"]:
        if p.get("error"):
            out.append(f"{p['params']}: {p['error']}")
        out += [f"{p['params']} {c['name']}: {c.get('measured_display')} vs {c['target_display']} {c.get('note') or ''}" for c in p["checks"] if not c["pass"]]
        out += [f"{p['params']} {e['text']}: {e['note']}" for e in p.get("envelope", []) if not e["pass"]]
    return out


def test_twelve_black_boxes_are_characterised(native):
    assert len(native) == 12
    for name, rep in native.items():
        assert rep["trust"] == "characterised", (name, failures(rep))
        assert len(rep["def_hash"]) == 64


@pytest.mark.skipif(not (WASM.exists() and NODE), reason="ngspice.wasm or node missing")
def test_browser_engine_gives_the_same_reports(prepared, native, tmp_path):
    order = [(name, i) for name, (_, _, runs) in prepared.items() for i in range(len(runs))]
    reqs = []
    for name, i in order:
        n = prepared[name][2][i]["netlist"]
        reqs.append({"netlist": n["text"], "includes": n["includes"], "hash": n["hash"]})
    (tmp_path / "requests.json").write_text(json.dumps(reqs), encoding="utf-8")
    subprocess.run([NODE, str(REPO / "tools/sim/run_wasm.mts"), str(tmp_path / "requests.json"), str(tmp_path / "results.json")],
                   check=True, cwd=REPO)
    results = json.loads((tmp_path / "results.json").read_text(encoding="utf-8"))
    outcomes: dict[str, list[dict]] = {}
    for (name, _), r in zip(order, results, strict=True):
        assert r["status"] == "ok", (name, r["log"][-1000:])
        outcomes.setdefault(name, []).append({"meas": r["meas"]})

    compared = 0
    for name, (d, s, runs) in prepared.items():
        wasm = bb.report(s, d, "wasm", runs, outcomes[name])
        assert wasm["trust"] == "characterised", (name, failures(wasm))
        for pn, pw in zip(native[name]["points"], wasm["points"], strict=True):
            assert pn["netlist_hash"] == pw["netlist_hash"]
            for cn, cw in zip(pn["checks"], pw["checks"], strict=True):
                assert cn["pass"] == cw["pass"], (name, pn["params"], cn["name"])
                assert cw["measured"] == pytest.approx(cn["measured"], rel=MEAS_REL, abs=1e-9), (name, pn["params"], cn["name"])
                compared += 1
    assert compared > 100
