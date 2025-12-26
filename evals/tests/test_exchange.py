"""The exchange provider: replies from a chat or a person, request by request, checked like an API's."""

from __future__ import annotations

import asyncio
import json
from pathlib import Path

import pytest

from exchange import AwaitingReply, Exchange, ReplyChanged, status
from run_evals import main
from test_run_evals import CHAIN_PLAN, USE_TEMPLATE, _same, rc
from tutor_api.llm.base import LlmRequest

CASES = [
    {"id": "chain", "prompt": "A 300 Hz sine into an RC low-pass at 2 kHz.", "level": "beginner",
     "expect": ["sine_source", "rc_lowpass"]},
    {"id": "oos", "prompt": "An Arduino thermometer.", "level": "beginner", "expect": "unsupported"},
]


def request(**kw) -> LlmRequest:
    return LlmRequest(**({"kind": "compose", "tier": "large", "system": "rules", "user": "Step: compose block b1",
                          "schema": {"type": "object"}, "max_tokens": 50} | kw))


def test_an_unanswered_call_waits_with_a_prompt_to_paste(tmp_path: Path):
    ex = Exchange(tmp_path, "test")
    req = request()
    with pytest.raises(AwaitingReply):
        asyncio.run(ex.complete(req))
    (pending,) = (tmp_path / "pending").glob("*.txt")
    text = pending.read_text(encoding="utf-8")
    assert pending.stem == f"compose-{req.key()[:12]}" and ex.pending == {pending.stem}
    assert f"replies/{pending.stem}.txt" in text and "temperature 0.2, at most 50 tokens, reply in one JSON object" in text
    # The messages a json_object endpoint gets: the schema appended to the user turn.
    assert text.endswith('SYSTEM:\nrules\n\nUSER:\nStep: compose block b1\n\nReply with one JSON object that matches '
                         'this JSON Schema:\n{"type": "object"}\n')
    assert status(tmp_path)["unanswered"] == [pending.stem]

    (tmp_path / "replies" / f"{pending.stem}.txt").write_text('```json\n{"use_template": true}\n```\n', encoding="utf-8")
    resp = asyncio.run(Exchange(tmp_path, "test").complete(req))
    assert resp.text == '```json\n{"use_template": true}\n```' and resp.model == "chat:test"
    assert resp.usage.estimated and resp.finish_reason == "stop"
    # A reply longer than the call's max_tokens is cut off, as an API would cut it.
    long = request(user="another", max_tokens=5)
    (tmp_path / "replies" / f"compose-{long.key()[:12]}.txt").write_text("x" * 40, encoding="utf-8")
    assert asyncio.run(Exchange(tmp_path).complete(long)).finish_reason == "length"


def test_a_used_reply_is_frozen(tmp_path: Path):
    req = request()
    reply = tmp_path / "replies" / f"compose-{req.key()[:12]}.txt"
    reply.parent.mkdir(parents=True)
    reply.write_text('{"use_template": true}', encoding="utf-8")
    asyncio.run(Exchange(tmp_path).complete(req))
    reply.write_text('{"use_template": false}', encoding="utf-8")
    with pytest.raises(ReplyChanged, match="tuning on the results"):
        Exchange(tmp_path)


def answer(name: str, text: str) -> str:
    """The test's model: a plan per request, a template for the source, a wrong pin first for the filter."""
    kind = name.split("-")[0]
    if kind == "plan":
        if "Arduino" in text:
            return json.dumps({"blocks": [], "links": [], "uncovered": ["a microcontroller"]})
        return json.dumps(CHAIN_PLAN)
    if kind == "narrate":
        return "A sine wave goes through a low-pass filter."
    if "Step: compose block b1" in text:
        return json.dumps(USE_TEMPLATE)
    if "Your previous attempt:" in text:
        assert "C1 (cap_film) has no pin X" in text  # the repair turn quotes the problem
        return json.dumps(rc())
    return json.dumps(rc("C1.X"))


def test_rounds_until_every_call_is_answered(databases, tmp_path: Path, monkeypatch, capsys):
    import stack

    monkeypatch.setattr(stack, "databases", lambda: _same(databases))
    prompts = tmp_path / "prompts.yaml"
    prompts.write_text(json.dumps(CASES), encoding="utf-8")
    ex = tmp_path / "exchange"
    args = ["--exchange", str(ex), "--label", "test model", "--prompts", str(prompts), "--gate"]

    rounds = []
    for n in range(1, 10):
        code = main(args + ["--out", str(tmp_path / f"round{n}")])
        waiting = status(ex)["unanswered"]
        rounds.append(sorted(w.split("-")[0] for w in waiting))
        if not waiting:
            break
        assert code == 3 and "waiting for a reply" in capsys.readouterr().out
        report = json.loads((tmp_path / f"round{n}" / "report.json").read_text(encoding="utf-8"))
        assert any("not complete" in p for p in report["gate"])
        for name in waiting:
            text = (ex / "pending" / f"{name}.txt").read_text(encoding="utf-8")
            (ex / "replies" / f"{name}.txt").write_text(answer(name, text), encoding="utf-8")

    # Plans; the source's compose, the narration and the out-of-scope re-plan; the filter's draft; its repair;
    # done. The second re-plan asks exactly what the first did (same plan, same problems), so the same reply answers it.
    assert rounds == [["plan", "plan"], ["compose", "narrate", "plan"], ["compose"], ["compose"], []]
    assert code == 0
    report = json.loads((tmp_path / f"round{len(rounds)}" / "report.json").read_text(encoding="utf-8"))
    m, rows = report["metrics"], {r["id"]: r for r in report["rows"]}
    assert report["gate"] == [] and m["awaiting"] == {"prompts": [], "calls": 0}
    assert [(b["template"], b["how"], b["attempts"], b["errors"]) for b in rows["chain"]["blocks"]] == [
        ("sine_source", "use_template", 1, []), ("rc_lowpass", "draft", 2, [["pin_not_found"]])]
    assert rows["oos"]["error"]["code"] == "unsupported_request"
    assert m["tokens_estimated"] and not m["latency_measured"]
    assert report["meta"]["provider"] == f"exchange: test model ({ex})"
    md = (tmp_path / f"round{len(rounds)}" / "report.md").read_text(encoding="utf-8")
    assert "(estimated: characters / 4)" in md and "not measured (no model time in this run)" in md
    assert len(json.loads((ex / "ledger.json").read_text(encoding="utf-8"))) == len(list((ex / "replies").glob("*.txt")))
