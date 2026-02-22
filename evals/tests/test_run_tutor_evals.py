"""The tutor harness end to end: cases asked through the real API (Postgres and Redis in
testcontainers), answered by scripted replies keyed on each case's question, checked (references,
grounding, the experiment re-simulated on the native ngspice) and graded by a scripted judge. Then
the same run recorded and replayed, which is what the CI gate does with the golden cassette."""

from __future__ import annotations

import asyncio
import json

from pytest import approx

import fixtures as fx
from run_evals import models
from run_tutor_evals import evaluate, load_cases
from tutor_api.llm.cassette import Replayer
from tutor_api.llm.fake import FakeProvider
from tutor_metrics import gate, summarize

TRY = {"ops": [{"op": "part.set_param", "body": {"refdes": "R1", "key": "resistance", "value": "36k"}},
               {"op": "part.set_param", "body": {"refdes": "R2", "key": "resistance", "value": "60k"}}],
       "predict": "fc drops to about 500 Hz"}
GRADE = {"correct": 4, "grounded": 5, "answers": 4, "level": 4, "mode": 5, "teaching": 4, "verdict": "pass", "notes": "Good."}
SCRIPT = {"rules": [
    {"kind": "ask", "when": ["How do I move the cutoff to 500 Hz?"],
     "reply": f"Doubling [R1] and [R2] halves fc: 996 Hz / 2 ≈ 498 Hz.\n\n```try\n{json.dumps(TRY)}\n```"},
    {"kind": "ask", "when": ["Why is R1 18k"], "reply": "[R1] (18 kΩ) and [C1] set the corner of [block:b2]; [R9] does not exist."},
    {"kind": "ask", "when": ["weather"], "reply": "I can only help with the circuit in your editor."},
    {"kind": "ask", "when": ["deleted R2"], "reply": "Without [R2] there is no feedback, so [net:B2_OUT] goes to a rail."},
    {"kind": "what_changed", "when": [], "reply": "Doubling [R1] moved fc from 996 Hz to 724 Hz, so it now FAILs; it guessed 3 mA."},
    {"kind": "tutor_judge", "when": [], "reply": GRADE},
]}
CASES = ["spec-sklp-500", "why-sklp-r1", "off-weather", "wc-sklp-r1-double", "pred-inv-remove-r2"]


def run(databases, provider, *, effort=None, record=False):
    m = models([provider], record=record)
    judge = models([provider], record=record)
    if record:
        judge.recorders[0].interactions = m.recorders[0].interactions
    cases = load_cases(fx.CASES, only=CASES)
    rows, _ = asyncio.run(evaluate(
        cases, m.gateway, database_url=databases[0], redis_url=databases[1],
        fixtures=json.loads(fx.FIXTURES.read_text(encoding="utf-8")), tiers={"ask": "small", "what_changed": "small"},
        effort=effort, concurrency=2, judge=judge.gateway, progress=lambda _: None))
    return m, judge, {r["id"]: r for r in rows}


def test_cases_are_asked_checked_and_graded(databases):
    fake = FakeProvider(SCRIPT)
    m, judge, rows = run(databases, fake, effort="high")

    spec = rows["spec-sklp-500"]
    assert spec["checks"]["try_valid"] and spec["tried"]["status"] == "ok"
    assert spec["checks"]["prediction"] == {"number": "500 Hz", "check": "b2 fc_hz", "measured": approx(499, abs=2), "held": True}
    assert spec["checks"]["ungrounded"] == [] and spec["model"] == "fake-small"
    why = rows["why-sklp-r1"]["checks"]
    assert (why["refs_valid"], why["invalid"], why["cite_missing"]) == (3, ["R9"], [])
    wc = rows["wc-sklp-r1-double"]
    assert [u["text"] for u in wc["checks"]["ungrounded"]] == ["3 mA"], "996 and 724 Hz are in the change summary"
    assert wc["context"]["text"].startswith("EDIT (rev 20 → 21):")
    assert all(r["rubric"] == GRADE for r in rows.values() if "rubric" in r["tags"])
    assert "rubric" not in rows["pred-inv-remove-r2"], "not in the sample"

    tutor_calls = [c for c in fake.calls if c.kind != "tutor_judge"]
    assert len(tutor_calls) == 5 and all(c.effort == "high" for c in tutor_calls)
    for r in rows.values():
        assert any(r["context"]["text"] in c.user for c in tutor_calls), f"{r['id']}: the harness reads the server's context"

    metrics = summarize(list(rows.values()), m.calls + judge.calls)
    assert metrics["refs_valid"] == approx(8 / 9) and metrics["arithmetic_errors"] == 0
    assert metrics["rubric"]["graded"] == 4 and metrics["calls"]["tutor_judge"]["calls"] == 4
    assert any("below 99%" in p for p in gate(metrics)), "R9 is an invalid reference"


def test_a_recorded_run_replays_to_the_same_answers(databases, tmp_path):
    m, _, recorded = run(databases, FakeProvider(SCRIPT), record=True)
    m.save_cassette(tmp_path / "cassette.json")
    m2, _, replayed = run(databases, Replayer.from_file(tmp_path / "cassette.json"))
    for rid, r in recorded.items():
        again = replayed[rid]
        assert (again["text"], again["answer"], again["checks"], again.get("rubric")) == (
            r["text"], r["answer"], r["checks"], r.get("rubric")), rid
    assert summarize(list(replayed.values()), m2.calls)["cassette_misses"] == 0
