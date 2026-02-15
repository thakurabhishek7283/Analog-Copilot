"""Ask over a real socket (LLD §5, §9): the answer streamed and read against the circuit at the
asked rev, what the model is sent, refusals before the stream starts, failures inside it, and
feedback."""

from __future__ import annotations

import dataclasses
import json
import uuid

import circuit_core as cc
import pytest
from sqlalchemy import func, select

from helpers import ask, create_project, envelopes, user_id
from tutor_api.db.tables import asks
from tutor_api.llm import prompts
from tutor_api.llm.base import ProviderError
from tutor_api.llm.fake import Delay
from tutor_api.models.contract import ApiError

TRY = {"ops": [{"op": "part.set_param", "body": {"refdes": "R1", "key": "resistance", "value": "2k"}}],
       "predict": "fc halves"}
REPLY = f"[R1] and [C1] set the corner on [net:B1_OUT]; [R7] is not here.\n\n```try\n{json.dumps(TRY)}\n```"
R1 = {"kind": "part", "refdes": "R1"}


@pytest.fixture(autouse=True)
def fresh_tutor(tutor_llm):
    tutor_llm.script.clear()
    tutor_llm.rules.clear()
    tutor_llm.calls.clear()


async def rc_project(http, headers, app) -> tuple[str, int]:
    """A project holding an RC low-pass block (R1, C1, net B1_OUT), inserted as the editor does."""
    pid = await create_project(http, headers)
    s = cc.Session(app.state.regs.current)
    ins = cc.unwrap(s.insert_block(json.dumps({"template": "rc_lowpass"})))
    r = await http.post(f"/v1/projects/{pid}/ops", json={"base_rev": 0, "ops": envelopes(ins["ops"], 0, "template")},
                        headers=headers)
    assert r.status_code == 200, r.text
    return pid, r.json()["rev"]


async def saved(app, ask_id: str):
    async with app.state.engine.connect() as conn:
        return (await conn.execute(select(asks).where(asks.c.id == uuid.UUID(ask_id)))).one()


async def count_asks(app, pid: str) -> int:
    async with app.state.engine.connect() as conn:
        return (await conn.execute(select(func.count()).where(asks.c.project_id == uuid.UUID(pid)))).scalar_one()


def text_of(events) -> str:
    return "".join(e["data"]["text"] for e in events if e["event"] == "answer.delta")


async def test_an_answer_streams_is_read_against_the_circuit_and_saved(http, alice, app, tutor_llm):
    pid, rev = await rc_project(http, alice, app)
    tutor_llm.script["ask"] = [REPLY]
    fc = {"block": "b1", "name": "fc_hz", "label": "Cutoff (−3 dB)", "symbol": "fc", "unit": "hertz", "target": 1000.0,
          "tol_pct": 10.0, "measured": 1250.0, "pass": False, "target_display": "1kHz", "measured_display": "1.25kHz"}
    sim = {"status": "ok", "op_v": {"B1_OUT": 0.5}, "checks": [fc]}
    body = {"question": "  Why is R1 1k? ", "selection": R1, "rev": rev, "sim": sim}
    events = await ask(http, pid, alice, body)

    names = [e["event"] for e in events]
    assert names[-1] == "answer.done" and set(names[:-1]) == {"answer.delta"} and len(names) > 3
    assert text_of(events) == REPLY
    done = events[-1]["data"]
    answer = done["answer"]
    assert answer["body"] == "[R1] and [C1] set the corner on [net:B1_OUT]; [R7] is not here."
    assert (answer["refs_valid"], answer["refs_invalid"]) == (3, 1)
    assert answer["try"]["problems"] == [] and answer["try"]["predict"] == "fc halves"
    assert done["usage"]["in_tokens"] > 0 and done["usage"]["out_tokens"] > 0

    (call,) = tutor_llm.calls
    assert (call.kind, call.tier, call.system, call.effort) == ("ask", "small", prompts.TUTOR_SYSTEM, None)
    assert call.timeout_s == 120.0, "longer than a generation call: latency is not a goal for Ask"
    assert "Student level: beginner." in call.user and "Mode: explain." in call.user
    assert "SELECTED: part R1" in call.user and "B1_OUT: C1.1 R1.2 | op 500mV" in call.user
    assert "measured 1.25kHz FAIL" in call.user, "the browser's check results reach the core under their wire names"
    assert call.user.endswith("The student asks:\n<<<\nWhy is R1 1k?\n>>>\n")

    row = await saved(app, done["ask_id"])
    assert (row.question, row.answer, row.rev, row.selection, row.mode) == ("Why is R1 1k?", REPLY, rev, R1, "explain")
    assert (row.refs_valid, row.refs_invalid, row.model, row.feedback) == (3, 1, "fake-small", None)
    assert str(row.user_id) == user_id(alice) and str(row.project_id) == pid
    assert (row.in_tokens, row.out_tokens) == (done["usage"]["in_tokens"], done["usage"]["out_tokens"])


async def test_level_mode_and_effort_reach_the_model(http, alice, app, tutor_llm):
    pid, rev = await rc_project(http, alice, app)
    tutor_llm.script["ask"] = ["What happens to the current through [R1] as the frequency rises?"]
    events = await ask(http, pid, alice, {"question": "Why does it roll off?", "rev": rev, "level": "advanced",
                                          "mode": "socratic", "effort": "high"})
    (call,) = tutor_llm.calls
    assert "Student level: advanced." in call.user and "Mode: Socratic." in call.user
    assert call.effort == "high", "the learner picks how hard a reasoning model thinks"
    assert "SELECTED: nothing" in call.user
    row = await saved(app, events[-1]["data"]["ask_id"])
    assert (row.mode, row.selection) == ("socratic", None)


async def test_the_answer_is_about_the_circuit_at_the_asked_rev(http, alice, app, tutor_llm):
    pid, rev = await rc_project(http, alice, app)  # the test settings snapshot every 5 ops: rev is past it
    add_r9 = [{"op": "part.add", "body": {"refdes": "R9", "part": "resistor_th"}}]
    r = await http.post(f"/v1/projects/{pid}/ops", json={"base_rev": rev, "ops": envelopes(add_r9, rev)}, headers=alice)
    assert r.status_code == 200, r.text

    async def answer_at(at: int) -> tuple[dict, str]:
        tutor_llm.calls.clear()
        tutor_llm.script["ask"] = ["[R1] and [R9]."]
        events = await ask(http, pid, alice, {"question": "What are R1 and R9 for?", "rev": at})
        return events[-1]["data"]["answer"], tutor_llm.calls[0].user

    head, prompt = await answer_at(rev + 1)  # the snapshot, then the ops after it
    assert (head["refs_valid"], head["refs_invalid"]) == (2, 0) and "  R9 resistor_th" in prompt
    before, prompt = await answer_at(rev)  # the snapshot itself
    assert (before["refs_valid"], before["refs_invalid"]) == (1, 1) and "R9" not in prompt.split("The student asks")[0]
    empty, prompt = await answer_at(0)  # before the snapshot: the op log folded from the start
    assert (empty["refs_valid"], empty["refs_invalid"]) == (0, 2) and "PARTS:" not in prompt


async def test_refusals_come_before_the_stream_and_ask_no_model(http, alice, bob, app, tutor_llm):
    pid, rev = await rc_project(http, alice, app)

    async def refused(body: dict, headers=alice) -> tuple[int, ApiError]:
        r = await http.post(f"/v1/projects/{pid}/ask", json=body, headers=headers)
        return r.status_code, ApiError.model_validate(r.json())

    status, e = await refused({"question": "Why?", "rev": rev + 1})
    assert (status, e.code, e.retryable) == (409, "rev_not_synced", True)
    status, e = await refused({"question": "Why?", "rev": rev, "selection": {"kind": "part", "refdes": "R7"}})
    assert (status, e.code) == (422, "part_not_found")
    status, e = await refused({"question": "Why?", "rev": rev, "selection": {"kind": "net", "id": "N_NOPE"}})
    assert (status, e.code) == (422, "net_not_found")
    for question in ("   ", "x" * 1001):
        status, e = await refused({"question": question, "rev": rev})
        assert (status, e.code) == (422, "invalid_request")
    status, e = await refused({"question": "Why?", "rev": rev, "effort": "medium"})
    assert (status, e.code) == (422, "invalid_request"), "low or high"
    status, e = await refused({"question": "Why?", "rev": rev, "netlist": "R1 1 0 1k"})
    assert (status, e.code) == (422, "invalid_request"), "the server never takes a client's circuit"
    status, e = await refused({"question": "Why?", "rev": rev}, headers=bob)
    assert (status, e.code) == (404, "not_found")

    tutor = app.state.tutor
    app.state.tutor = None
    try:
        status, e = await refused({"question": "Why?", "rev": rev})
        assert (status, e.code) == (503, "tutor_unavailable")
    finally:
        app.state.tutor = tutor
    assert tutor_llm.calls == [] and await count_asks(app, pid) == 0


async def test_a_failed_answer_ends_the_stream_with_an_error_and_is_not_saved(http, alice, app, tutor_llm):
    pid, rev = await rc_project(http, alice, app)
    tutor_llm.script["ask"] = [ProviderError("bad_request", "refused", retryable=False)]
    events = await ask(http, pid, alice, {"question": "Why?", "rev": rev})
    assert [e["event"] for e in events] == ["error"]
    assert events[0]["data"]["code"] == "llm_unavailable" and events[0]["data"]["retryable"] is True

    settings = app.state.settings
    app.state.settings = dataclasses.replace(settings, ask_timeout_s=0.2)
    try:
        tutor_llm.script["ask"] = [Delay(5.0, "too late")]
        events = await ask(http, pid, alice, {"question": "Why?", "rev": rev})
    finally:
        app.state.settings = settings
    assert [(e["event"], e["data"]["code"]) for e in events] == [("error", "tutor_timeout")]
    assert await count_asks(app, pid) == 0


async def test_feedback_is_stored_for_the_asker_only(http, alice, bob, app, tutor_llm):
    pid, rev = await rc_project(http, alice, app)
    tutor_llm.script["ask"] = ["See [R1]."]
    ask_id = (await ask(http, pid, alice, {"question": "Why?", "rev": rev}))[-1]["data"]["ask_id"]

    async def give(value, headers=alice, aid=ask_id):
        return await http.post(f"/v1/asks/{aid}/feedback", json={"feedback": value}, headers=headers)

    assert (await give(1)).status_code == 204 and (await saved(app, ask_id)).feedback == 1
    assert (await give(-1)).status_code == 204 and (await saved(app, ask_id)).feedback == -1, "a change of mind"
    r = await give(0)
    assert r.status_code == 422 and ApiError.model_validate(r.json()).code == "invalid_request"
    for r in (await give(1, headers=bob), await give(1, aid=str(uuid.uuid4())), await give(1, aid="nope")):
        assert r.status_code == 404 and ApiError.model_validate(r.json()).code == "not_found"
    assert (await saved(app, ask_id)).feedback == -1


# ---------------------------------------------------------------- what changed

def fc_check(measured: float, display: str, ok: bool) -> dict:
    return {"block": "b1", "name": "fc_hz", "label": "Cutoff (−3 dB)", "symbol": "fc", "unit": "hertz", "target": 1000.0,
            "tol_pct": 10.0, "measured": measured, "pass": ok, "target_display": "1kHz", "measured_display": display}


async def edited_project(http, headers, app) -> tuple[str, int, int]:
    """`rc_project`, then R1 set to 2k: (project, rev before the edit, rev after it)."""
    pid, rev = await rc_project(http, headers, app)
    set_r1 = [{"op": "part.set_param", "body": {"refdes": "R1", "key": "resistance", "value": "2k"}}]
    r = await http.post(f"/v1/projects/{pid}/ops", json={"base_rev": rev, "ops": envelopes(set_r1, rev)}, headers=headers)
    assert r.status_code == 200, r.text
    return pid, rev, r.json()["rev"]


async def test_what_changed_explains_the_edit_from_both_simulations_and_is_saved(http, alice, app, tutor_llm):
    pid, before, after = await edited_project(http, alice, app)
    reply = "Making [R1] smaller raised the cutoff of [block:b1], so [net:B1_OUT] passes more."
    tutor_llm.script["what_changed"] = [reply]
    body = {"from_rev": before, "rev": after, "mode": "socratic", "effort": "high",
            "before": {"status": "ok", "op_v": {"B1_OUT": 0.5}, "checks": [fc_check(995.0, "995Hz", True)]},
            "after": {"status": "ok", "op_v": {"B1_OUT": 0.25}, "checks": [fc_check(7960.0, "7.96kHz", False)]}}
    events = await ask(http, pid, alice, body, path="what-changed")
    assert events[-1]["event"] == "answer.done" and text_of(events) == reply
    done = events[-1]["data"]
    assert (done["answer"]["refs_valid"], done["answer"]["refs_invalid"]) == (3, 0)

    (call,) = tutor_llm.calls
    assert (call.kind, call.tier, call.system, call.effort) == ("what_changed", "small", prompts.TUTOR_SYSTEM, "high")
    assert call.timeout_s == 120.0 and "Mode: Socratic." in call.user
    assert f"EDIT (rev {before} → {after}):" in call.user and "R1 [b1]: resistance" in call.user and "→ 2kΩ" in call.user
    assert "b1 fc_hz (1kHz ±10%): 995Hz pass → 7.96kHz FAIL" in call.user
    assert "B1_OUT: op 500mV → 250mV" in call.user
    assert "C1 " not in call.user.split("EDIT")[1].split("BLOCK")[0], "only what the edit touched, not the circuit"

    row = await saved(app, done["ask_id"])
    assert (row.kind, row.from_rev, row.rev, row.question, row.selection, row.mode) == (
        "what_changed", before, after, "What changed?", None, "socratic")
    assert (row.answer, row.refs_valid, row.model) == (reply, 3, "fake-small")
    r = await http.post(f"/v1/asks/{done['ask_id']}/feedback", json={"feedback": 1}, headers=alice)
    assert r.status_code == 204 and (await saved(app, done["ask_id"])).feedback == 1


async def test_what_changed_refusals_come_before_the_stream(http, alice, bob, app, tutor_llm):
    pid, before, after = await edited_project(http, alice, app)
    ok = {"from_rev": before, "rev": after, "before": {}, "after": {}}

    async def refused(body: dict, headers=alice) -> tuple[int, ApiError]:
        r = await http.post(f"/v1/projects/{pid}/what-changed", json=body, headers=headers)
        return r.status_code, ApiError.model_validate(r.json())

    for from_rev, rev in ((after, after), (after, before)):
        status, e = await refused({**ok, "from_rev": from_rev, "rev": rev})
        assert (status, e.code) == (422, "invalid_request")
    status, e = await refused({**ok, "rev": after + 1})
    assert (status, e.code, e.retryable) == (409, "rev_not_synced", True)
    status, e = await refused({**ok, "question": "Why?"})
    assert (status, e.code) == (422, "invalid_request"), "a change request has no question"
    status, e = await refused({k: v for k, v in ok.items() if k != "after"})
    assert (status, e.code) == (422, "invalid_request")
    status, e = await refused(ok, headers=bob)
    assert (status, e.code) == (404, "not_found")
    tutor = app.state.tutor
    app.state.tutor = None
    try:
        status, e = await refused(ok)
        assert (status, e.code) == (503, "tutor_unavailable")
    finally:
        app.state.tutor = tutor
    assert tutor_llm.calls == [] and await count_asks(app, pid) == 0
