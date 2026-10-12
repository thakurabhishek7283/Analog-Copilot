"""Characterise the reference black boxes (tools/sim/blackboxes.yaml) on an engine.

Shared by test_blackboxes.py (native and browser engines). Any other engine, such as the Linux
ngspice in the sim-runner image, simulates the bench netlists written by `requests` and is
compared with the native engine by `compare`:

    python tools/sim/blackboxes.py requests requests.json
    docker run --rm -v "$PWD/tools/spikes/p5_0:/spike" -v "$PWD/out:/work" \
        analog-copilot/sim-runner:dev python /spike/run_native.py /work/requests.json /work/linux.json 1
    python tools/sim/blackboxes.py compare linux out/linux.json
"""

from __future__ import annotations

import json
import sys
from pathlib import Path

import yaml

REPO = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(REPO / "workers" / "sim_runner"))

import circuit_core as cc  # noqa: E402

FILE = REPO / "tools" / "sim" / "blackboxes.yaml"


def definitions() -> dict[str, dict]:
    """Black box name -> ComponentDef, `@path` model texts read from the repository."""
    out = {}
    for name, d in yaml.safe_load(FILE.read_text(encoding="utf-8")).items():
        text = d.get("model", {}).get("text")
        if isinstance(text, str) and text.startswith("@"):
            d["model"]["text"] = (REPO / text[1:]).read_text(encoding="utf-8")
        out[name] = d
    return out


def session(reg: cc.Registry, d: dict) -> cc.Session:
    s = cc.Session(reg, None)
    cc.unwrap(s.apply_ops(json.dumps([{"op": "lib.add", "body": {"def": d}}]), "user"))
    return s


def benches(reg: cc.Registry, d: dict) -> tuple[cc.Session, list[dict]]:
    s = session(reg, d)
    return s, cc.unwrap(s.characterise_benches(d["part"]["id"]))


def report(s: cc.Session, d: dict, ngspice: str, runs: list[dict], outcomes: list[dict]) -> dict:
    return cc.unwrap(s.characterise_report(d["part"]["id"], ngspice, json.dumps(runs), json.dumps(outcomes)))


def native_outcomes(runs: list[dict]) -> list[dict]:
    from sim_runner.ngspice_batch import simulate

    out = []
    for r in runs:
        n = r.get("netlist")
        if n is None:
            out.append({"meas": {}, "error": r.get("error")})
            continue
        res = simulate(n["text"], n["includes"], hash=n["hash"], vectors=False, timeout_s=60)
        out.append({"meas": res.meas, **({"error": f"{res.status}: {res.log[-300:]}"} if res.status != "ok" else {})})
    return out


def characterise_native(ngspice: str = "native") -> dict[str, dict]:
    from sim_runner.ngspice_batch import REGISTRY_DIR

    reg = cc.load_registry_dir(REGISTRY_DIR)
    reports = {}
    for name, d in definitions().items():
        s, runs = benches(reg, d)
        reports[name] = report(s, d, ngspice, runs, native_outcomes(runs))
    return reports


def prepared() -> dict[str, tuple[dict, cc.Session, list[dict]]]:
    from sim_runner.ngspice_batch import REGISTRY_DIR

    reg = cc.load_registry_dir(REGISTRY_DIR)
    out = {}
    for name, d in definitions().items():
        s, runs = benches(reg, d)
        out[name] = (d, s, runs)
    return out


def write_requests(path: str) -> None:
    reqs = []
    for name, (_, _, runs) in prepared().items():
        for i, r in enumerate(runs):
            n = r["netlist"]
            reqs.append({"name": f"{name}/{i}", "netlist": n["text"], "includes": n["includes"], "hash": n["hash"]})
    Path(path).write_text(json.dumps(reqs), encoding="utf-8")


def compare(engine: str, results_path: str) -> int:
    """Reports from another engine's results, against the native engine's: problems found."""
    results = {r["name"]: r for r in json.loads(Path(results_path).read_text(encoding="utf-8"))}
    problems = 0
    worst = 0.0
    for name, (d, s, runs) in prepared().items():
        theirs = [{"meas": results[f"{name}/{i}"]["meas"]} for i in range(len(runs))]
        other = report(s, d, engine, runs, theirs)
        mine = report(s, d, "native", runs, native_outcomes(runs))
        for pm, po in zip(mine["points"], other["points"], strict=True):
            for cm, co in zip(pm["checks"], po["checks"], strict=True):
                a, b = cm.get("measured"), co.get("measured")
                rel = abs(a - b) / max(abs(a), 1e-9) if a is not None and b is not None else float("inf")
                worst = max(worst, rel)
                if cm["pass"] != co["pass"] or rel > 1e-3:
                    problems += 1
                    print(f"{name} {pm['params']} {cm['name']}: native {a} ({cm['pass']}), {engine} {b} ({co['pass']})")
        print(f"{name}: {other['trust']} on {engine}")
    print(f"worst relative difference {worst:.2e}; {problems} problems")
    return problems


if __name__ == "__main__":
    cmd = sys.argv[1] if len(sys.argv) > 1 else "native"
    if cmd == "requests":
        write_requests(sys.argv[2])
    elif cmd == "compare":
        sys.exit(1 if compare(sys.argv[2], sys.argv[3]) else 0)
    else:
        json.dump(characterise_native(cmd), sys.stdout, indent=1)
