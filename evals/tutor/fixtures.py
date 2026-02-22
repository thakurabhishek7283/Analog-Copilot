"""The tutor evals' circuits, frozen with their simulation values (LLD §9, §15).

    python evals/tutor/fixtures.py          # rebuild fixtures.json from circuits.yaml and cases.yaml

Each circuit in circuits.yaml is built with circuit-core as the editor builds it (`insert` is the
Insert block form, one batch by author `template`; `ops` are the student's own edits), and each
"What changed?" case's edit is applied on top. Every resulting circuit is then simulated the way the
editor simulates it: sim_values.mts runs the snapshot through the editor's own store and simulation
scheduler on ngspice.wasm, with the core's spec checks, and `simValues` makes what a question sends.

The values are frozen into fixtures.json, so every run (and every OS) sends the model the same text:
a recorded run replays anywhere. Rebuild after a change to the circuits, the cases' edits, the registry
or the editor's simulation; evals/tests/test_tutor_fixtures.py fails until then.
"""

from __future__ import annotations

import json
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path
from typing import Any

import circuit_core as cc
import yaml

HERE = Path(__file__).resolve().parent
REPO = HERE.parents[1]
CIRCUITS = HERE / "circuits.yaml"
CASES = HERE / "cases.yaml"
FIXTURES = HERE / "fixtures.json"


def load_yaml(path: Path) -> list[dict[str, Any]]:
    return yaml.safe_load(path.read_text(encoding="utf-8")) if path.exists() else []


def registry() -> cc.Registry:
    return cc.load_registry_dir(REPO / "registry")


def build(reg: cc.Registry, steps: list[dict[str, Any]], session: cc.Session | None = None) -> tuple[cc.Session, list[dict[str, Any]]]:
    """Apply `steps` (each `{insert: InsertBlock}` or `{ops: [...]}`) to `session` (default a new
    circuit): the session and its batches, `{author, ops}` each, in order."""
    s = session or cc.Session(reg)
    batches = []
    for step in steps:
        if "insert" in step:
            ops = cc.unwrap(s.insert_block(json.dumps(step["insert"])))["ops"]
            author = "template"
        else:
            ops, author = step["ops"], "user"
        cc.unwrap(s.apply_ops(json.dumps(ops), author))
        batches.append({"author": author, "ops": ops})
    return s, batches


def envelopes(batches: list[dict[str, Any]], base_rev: int = 0) -> list[dict[str, Any]]:
    """The batches as consecutive op envelopes from `base_rev`, as the editor syncs them."""
    out = []
    for b in batches:
        for op in b["ops"]:
            rev = base_rev + len(out)
            out.append({"v": 1, "seq": rev + 1, **op, "author": b["author"], "base_rev": rev})
    return out


def session_at(reg: cc.Registry, batches: list[dict[str, Any]]) -> cc.Session:
    """A circuit replayed from its batches, as the server folds its op log."""
    s = cc.Session(reg)
    for b in batches:
        cc.unwrap(s.apply_ops(json.dumps(b["ops"]), b["author"]))
    return s


def structure(circuits: list[dict[str, Any]], cases: list[dict[str, Any]], reg: cc.Registry) -> dict[str, Any]:
    """fixtures.json without the simulation values: what circuits.yaml and the cases' edits build.
    Also the sessions to simulate, by key (`circuit:<id>`, `edit:<case id>`)."""
    out: dict[str, Any] = {"registry_version": reg.version, "circuits": {}, "edits": {}}
    sessions: dict[str, cc.Session | None] = {}
    by_id = {}
    for c in circuits:
        try:
            s, batches = build(reg, c["steps"])
        except cc.OpRejected as e:
            raise SystemExit(f"circuit {c['id']}: {e}") from None
        by_id[c["id"]] = batches
        out["circuits"][c["id"]] = {"title": c["title"], "batches": batches, "rev": s.rev, "sim": None}
        sessions[f"circuit:{c['id']}"] = None if c.get("sim") == "none" else s
    for case in cases:
        if case.get("kind") != "what_changed":
            continue
        base = session_at(reg, by_id[case["circuit"]])
        try:
            s, batches = build(reg, [{"ops": case["edit"]}], base)
        except cc.OpRejected as e:
            raise SystemExit(f"case {case['id']}: its edit is refused: {e}") from None
        out["edits"][case["id"]] = {"ops": batches[0]["ops"], "rev": s.rev, "sim": None}
        sessions[f"edit:{case['id']}"] = s
    return {"fixtures": out, "sessions": sessions}


def simulate(snapshots: list[str]) -> list[dict[str, Any]]:
    """Each snapshot simulated the way the editor does it (sim_values.mts on ngspice.wasm)."""
    node = shutil.which("node")
    if node is None:
        raise SystemExit("node is required (the editor's simulation runs in Node)")
    with tempfile.TemporaryDirectory() as tmp:
        src, dst = Path(tmp) / "snapshots.json", Path(tmp) / "values.json"
        src.write_text(json.dumps(snapshots), encoding="utf-8")
        subprocess.run([node, str(HERE / "sim_values.mts"), str(src), str(dst)], check=True, cwd=REPO)
        return json.loads(dst.read_text(encoding="utf-8"))


def main() -> int:
    reg = registry()
    built = structure(load_yaml(CIRCUITS), load_yaml(CASES), reg)
    fx, sessions = built["fixtures"], built["sessions"]
    keys = [k for k, s in sessions.items() if s is not None]
    values = dict(zip(keys, simulate([sessions[k].snapshot() for k in keys]), strict=True))
    for key, value in values.items():
        kind, _, name = key.partition(":")
        fx["circuits" if kind == "circuit" else "edits"][name]["sim"] = value
    for name, c in fx["circuits"].items():
        if c["sim"] is None:
            c["sim"] = {}
    FIXTURES.write_text(json.dumps(fx, indent=1, ensure_ascii=False) + "\n", encoding="utf-8", newline="\n")
    bad = {k: v.get("status") for k, v in values.items() if v.get("status") != "ok"}
    print(f"{len(fx['circuits'])} circuits and {len(fx['edits'])} edits, {len(values)} simulated; "
          f"not ok: {bad or 'none'}; wrote {FIXTURES.relative_to(REPO)}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
