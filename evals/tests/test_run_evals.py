"""The harness end to end: real jobs on the real API (Postgres and Redis in testcontainers, the sim
worker on the native ngspice), with scripted model replies keyed on a word in each prompt. Then the
same run recorded and replayed, which is what the CI gate does with the golden cassette."""

from __future__ import annotations

import asyncio
import json
from pathlib import Path
from typing import Any

from metrics import gate, summarize
from run_evals import evaluate, main, models
from tutor_api.llm.cassette import Replayer
from tutor_api.llm.fake import FakeProvider


def rc_plan() -> dict[str, Any]:
    return {"blocks": [{"id": "b1", "template": "rc_lowpass", "title": "Low-pass filter", "purpose": "R1 and C1 filter.",
                        "targets": [{"name": "fc_hz", "value": "2k"}]}], "links": [], "uncovered": []}


CHAIN_PLAN = {
    "blocks": [
        {"id": "b1", "template": "sine_source", "title": "Test signal", "purpose": "V1 makes a sine.",
         "targets": [{"name": "freq_hz", "value": "300"}]},
        {"id": "b2", "template": "rc_lowpass", "title": "Low-pass filter", "purpose": "R1 and C1 filter.",
         "targets": [{"name": "fc_hz", "value": "2k"}]},
    ],
    "links": [{"source": "b1.out", "target": "b2.in"}],
    "uncovered": [],
}
USE_TEMPLATE = {"use_template": True, "parts": [], "nets": []}


def rc(cap_pin: str = "C1.1") -> dict[str, Any]:
    """An RC low-pass draft, 8.2k and 10n (fc 1.94 kHz); a bad `cap_pin` is a wiring error."""
    return {
        "use_template": False,
        "parts": [{"ref": "R1", "part": "resistor_th", "params": [{"name": "resistance", "value": "8.2k"}]},
                  {"ref": "C1", "part": "cap_film", "params": [{"name": "capacitance", "value": "10n"}]}],
        "nets": [{"name": "in", "pins": ["R1.1"]}, {"name": "out", "pins": ["R1.2", cap_pin]},
                 {"name": "gnd", "pins": ["C1.2"]}],
    }


SCRIPT = {
    "rules": [
        {"kind": "plan", "when": ["eval-oos"], "reply": {"blocks": [], "links": [], "uncovered": ["a microcontroller"]}},
        {"kind": "plan", "when": ["eval-chain"], "reply": CHAIN_PLAN},
        {"kind": "plan", "when": [], "reply": rc_plan()},
        {"kind": "narrate", "when": [], "reply": "A filter. The resistor and the capacitor share the signal."},
        {"kind": "compose", "when": ["eval-tpl"], "reply": USE_TEMPLATE},
        {"kind": "compose", "when": ["eval-fallback"], "reply": rc("C1.X")},
        {"kind": "compose", "when": ["eval-chain", "Step: compose block b1"], "reply": USE_TEMPLATE},
        {"kind": "compose", "when": ["eval-chain"], "reply": rc()},
        {"kind": "compose", "when": ["eval-repair", "Your previous attempt:"], "reply": rc()},
        {"kind": "compose", "when": ["eval-repair"], "reply": rc("C1.X")},
    ]
}

CASES = [
    {"id": "repair", "prompt": "eval-repair: an RC low-pass at 2 kHz", "level": "beginner", "expect": ["rc_lowpass"]},
    {"id": "fallback", "prompt": "eval-fallback: an RC low-pass at 2 kHz", "level": "intermediate",
     "expect": ["rc_lowpass"]},
    {"id": "tpl", "prompt": "eval-tpl: an RC high-pass", "level": "advanced", "expect": ["rc_highpass"]},
    {"id": "chain", "prompt": "eval-chain: a sine into an RC low-pass", "level": "beginner",
     "expect": ["sine_source", "rc_lowpass"]},
    {"id": "oos", "prompt": "eval-oos: an Arduino thermometer", "level": "beginner", "expect": "unsupported"},
]


def run(databases, provider, *, record: bool = False, cases=CASES):
    m = models([provider], record=record)
    rows, version = asyncio.run(evaluate(cases, m.gateway, database_url=databases[0], redis_url=databases[1],
                                         concurrency=3, progress=lambda _: None))
    return m, rows, version


def outcomes(rows) -> dict[str, Any]:
    return {r["id"]: (r["state"], (r["error"] or {}).get("code"), r["tokens"],
                      [(b["template"], b["how"], b["attempts"], b["errors"]) for b in r["blocks"]]) for r in rows}


def test_a_run_measures_every_outcome(databases, tmp_path: Path):
    m, rows, version = run(databases, FakeProvider(SCRIPT), record=True)
    assert version == "2025.09.0"
    by_id = {r["id"]: r for r in rows}
    assert [r["id"] for r in rows] == [c["id"] for c in CASES]

    assert outcomes(rows)["repair"][3] == [("rc_lowpass", "draft", 2, [["pin_not_found"]])]
    assert outcomes(rows)["fallback"][3] == [("rc_lowpass", "fallback", 3, [["pin_not_found"]] * 3)]
    assert outcomes(rows)["tpl"][3] == [("rc_lowpass", "use_template", 1, [])]
    assert outcomes(rows)["chain"][3] == [("sine_source", "use_template", 1, []), ("rc_lowpass", "draft", 1, [])]
    assert by_id["oos"]["state"] == "failed" and by_id["oos"]["error"]["code"] == "unsupported_request"
    assert by_id["oos"]["plan"] is None and by_id["oos"]["blocks"] == []

    rc_check = by_id["chain"]["blocks"][1]["checks"][0]
    assert rc_check["name"] == "fc_hz" and rc_check["pass"] and abs(rc_check["measured"] - 1941) / 1941 < 0.03
    assert len(by_id["fallback"]["blocks"][0]["attempt_ms"]) == 3
    t = by_id["chain"]["times"]
    assert 0 < t["first_narration"] <= t["first_op"] <= t["total"]
    assert by_id["chain"]["tokens"]["in"] > 0 and by_id["chain"]["model"] == "fake-large"

    s = summarize(rows, m.calls)
    assert (s["done"], s["in_scope"], s["out_of_scope_refused"]) == (4, 4, 1)
    assert s["blocks"] == 5 and s["commit_without_fallback"] == 0.8
    assert s["first_attempt_pass"] == 0.6 and s["mean_attempts"] == 1.6
    assert s["plan_match"] == 0.75  # tpl asked for a high-pass and got the low-pass plan
    assert s["attempt_errors"] == {"pin_not_found": 4}
    # The refused plan is asked twice more first (re-plans), like any plan with problems.
    assert s["calls"]["compose"]["calls"] == 8 and s["calls"]["plan"]["calls"] == 4 + 3
    assert s["calls"]["narrate"]["calls"] == 4  # not for the refused plan
    assert gate(s) == [] and gate(s, s | {"commit_without_fallback": 0.9})

    # Replaying the recording reproduces the run: what the CI gate relies on.
    cassette = tmp_path / "cassette.json"
    m.save_cassette(cassette)
    replay, again, _ = run(databases, Replayer.from_file(cassette))
    assert outcomes(again) == outcomes(rows)
    s2 = summarize(again, replay.calls)
    assert {k: s2[k] for k in ("commit_without_fallback", "mean_attempts", "tokens_in_mean", "tokens_out_mean")} == \
           {k: s[k] for k in ("commit_without_fallback", "mean_attempts", "tokens_in_mean", "tokens_out_mean")}
    assert gate(s2, s) == []

    # A prompt the cassette has not seen fails the gate, not just a block.
    changed = [CASES[0] | {"prompt": CASES[0]["prompt"] + " please"}]
    replay, missed, _ = run(databases, Replayer.from_file(cassette), cases=changed)
    s3 = summarize(missed, replay.calls)
    assert s3["cassette_misses"]["prompts"] == ["repair"]
    assert "not in the cassette" in gate(s3, s)[0]


def test_the_command_line(databases, tmp_path: Path, monkeypatch, capsys):
    """`run_evals.py --fake ... --record --gate`, on the containers the test already has."""
    import stack

    monkeypatch.setattr(stack, "databases", lambda: _same(databases))
    script = tmp_path / "script.json"
    script.write_text(json.dumps(SCRIPT), encoding="utf-8")
    prompts = tmp_path / "prompts.yaml"
    prompts.write_text(json.dumps(CASES), encoding="utf-8")  # JSON is YAML
    out = tmp_path / "run"

    assert main(["--fake", str(script), "--prompts", str(prompts), "--record", "--gate", "--out", str(out)]) == 0
    report = json.loads((out / "report.json").read_text(encoding="utf-8"))
    assert report["gate"] == [] and report["metrics"]["commit_without_fallback"] == 0.8
    assert report["meta"]["provider"] == f"fake ({script})" and len(report["rows"]) == 5
    assert "**Gate: passed.**" in (out / "report.md").read_text(encoding="utf-8")
    assert (out / "cassette.json").exists()
    assert "Phase 2 gate (LLD 16)" in capsys.readouterr().out

    # The replay against that run as its baseline, with a floor it cannot meet: exit status 1.
    code = main(["--replay", str(out / "cassette.json"), "--prompts", str(prompts), "--gate", "--min-commit", "0.9",
                 "--baseline", str(out / "report.json"), "--out", str(tmp_path / "replay")])
    assert code == 1
    report = json.loads((tmp_path / "replay" / "report.json").read_text(encoding="utf-8"))
    assert report["gate"] == ["blocks committed without template fallback: 80.0%, below 90%"]


def test_a_real_provider_needs_live(monkeypatch):
    import pytest

    monkeypatch.setenv("LLM_PROVIDER", "gemini")
    with pytest.raises(SystemExit, match="pass --live"):
        main(["--limit", "1"])


class _same:
    """`stack.databases()` stand-in that hands out the session's containers."""

    def __init__(self, urls):
        self.urls = urls

    def __enter__(self):
        return self.urls

    def __exit__(self, *exc):
        return False


def test_rate_limits_are_waited_out_not_counted_against_the_model():
    import pytest

    from run_evals import Paced
    from tutor_api.llm.base import LlmRequest, LlmResponse, ProviderError

    class Limited:
        name = "limited"

        def __init__(self, errors):
            self.errors = list(errors)
            self.calls = 0

        async def complete(self, req):
            self.calls += 1
            if self.errors:
                raise self.errors.pop(0)
            return LlmResponse("{}", "m")

        async def stream(self, req, on_delta):
            await on_delta("Hello ")
            return await self.complete(req)

    req = LlmRequest("plan", "large", "s", "u")
    slow = lambda: ProviderError("rate_limited", "429", True, retry_after_s=0.001)  # noqa: E731
    inner = Limited([slow(), slow()])
    paced = Paced(inner, max_wait_s=60)
    assert asyncio.run(paced.complete(req)).text == "{}"
    assert inner.calls == 3 and paced.waits == 2
    # A daily quota is not waited out, and neither is a stream that has already sent text.
    with pytest.raises(ProviderError, match="quota_exhausted"):
        asyncio.run(Paced(Limited([ProviderError("quota_exhausted", "day", False)]), 60).complete(req))

    async def on_delta(_: str) -> None: ...

    with pytest.raises(ProviderError, match="rate_limited"):
        asyncio.run(Paced(Limited([slow()]), 60).stream(req, on_delta))
    # Past the per-call limit, the error goes on to the gateway as usual.
    with pytest.raises(ProviderError, match="rate_limited"):
        asyncio.run(Paced(Limited([slow()] * 50), max_wait_s=1.5).complete(req))
