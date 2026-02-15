"""The tutor service without a server (LLD §9, §14): the fixed safety note for mains-powered parts,
the model tier per request kind, and what "What changed?" sends the model."""

from __future__ import annotations

import json

import circuit_core as cc
import pytest

from tutor_api.config import REPO
from tutor_api.llm.config import tutor_tiers_from_env
from tutor_api.llm.fake import FakeProvider
from tutor_api.llm.gateway import Gateway
from tutor_api.llm import prompts
from tutor_api.models.contract import AskRequest, ChangeRequest
from tutor_api.tutor import Tutor
from tutor_api.tutor.service import safety_note


def registry(mains: bool) -> cc.Registry:
    bundle = json.loads(cc.load_registry_dir(REPO / "registry").to_json())
    if mains:  # no registry part is flagged yet: a test one stands in
        bundle["parts"]["vsource_sine"]["hazard"] = "mains"
    return cc.Registry.from_json(json.dumps(bundle))


def sine_into_rc(reg: cc.Registry) -> cc.Session:
    s = cc.Session(reg)
    for template in ("sine_source", "rc_lowpass"):
        ins = cc.unwrap(s.insert_block(json.dumps({"template": template})))
        cc.unwrap(s.apply_ops(json.dumps(ins["ops"]), "template"))
    return s


async def answer(reg: cc.Registry, reply: str) -> tuple[list[str], object]:
    s = sine_into_rc(reg)
    tutor = Tutor(Gateway(FakeProvider({"ask": [reply]})))
    req = AskRequest.model_validate({"question": "What does R1 do?", "rev": s.rev, "selection": {"kind": "part", "refdes": "R1"}})
    deltas: list[str] = []

    async def on_delta(text: str) -> None:
        deltas.append(text)

    context = tutor.context(s, req)
    asked = await tutor.answer(s, tutor.request(req, context), context["hazards"], on_delta)
    return deltas, asked


async def test_mains_parts_get_a_fixed_safety_note_citing_them():
    deltas, asked = await answer(registry(mains=True), "[R1] limits the current.")
    note = safety_note(["V1"])
    assert note.startswith("\n\nSafety: [V1] runs from mains voltage")
    assert asked.text == "[R1] limits the current." + note and "".join(deltas) == asked.text
    assert deltas[-1] == note, "streamed after the model's reply, as its own delta"
    assert asked.answer["refs_valid"] == 2 and asked.answer["body"].endswith("an isolation transformer.")
    assert safety_note(["V1", "T1"]).startswith("\n\nSafety: [V1], [T1] run from mains voltage")


async def test_no_note_without_mains_parts():
    _, asked = await answer(registry(mains=False), "[R1] limits the current.")
    assert asked.text == "[R1] limits the current."


def test_the_tutor_tiers_are_small_unless_configured():
    assert tutor_tiers_from_env({}) == {"ask": "small", "what_changed": "small"}
    assert tutor_tiers_from_env({"LLM_TIER_ASK": " Large "}) == {"ask": "large", "what_changed": "small"}
    assert tutor_tiers_from_env({"LLM_TIER_WHAT_CHANGED": "large"}) == {"ask": "small", "what_changed": "large"}
    with pytest.raises(RuntimeError, match="LLM_TIER_ASK"):
        tutor_tiers_from_env({"LLM_TIER_ASK": "medium"})
    s = sine_into_rc(registry(mains=False))
    tutor = Tutor(Gateway(FakeProvider({})), {"ask": "large"})
    req = AskRequest.model_validate({"question": "Why?", "rev": s.rev})
    assert tutor.request(req, tutor.context(s, req)).tier == "large"


def test_what_changed_sends_the_change_summary_with_the_same_system_prompt():
    reg = registry(mains=True)
    before = sine_into_rc(reg)
    after = cc.Session(reg, before.snapshot())
    cc.unwrap(after.apply_ops(json.dumps([{"op": "part.set_param", "body": {"refdes": "R1", "key": "resistance", "value": "2k"}}]), "user"))
    req = ChangeRequest.model_validate({"from_rev": before.rev, "rev": after.rev, "before": {"op_v": {"B2_OUT": 0.5}},
                                        "after": {"op_v": {"B2_OUT": 0.25}}, "mode": "socratic", "effort": "low"})
    tutor = Tutor(Gateway(FakeProvider({})), tutor_tiers_from_env({"LLM_TIER_WHAT_CHANGED": "large"}))
    context = tutor.changes(before, after, req)
    assert context["parts"] == ["R1"] and context["hazards"] == ["V1"]
    llm = tutor.change_request(req, context)
    assert (llm.kind, llm.tier, llm.system, llm.effort) == ("what_changed", "large", prompts.TUTOR_SYSTEM, "low")
    assert "Mode: Socratic." in llm.user and "R1 [b2]: resistance 16kΩ → 2kΩ" in llm.user
    assert "B2_OUT: op 500mV → 250mV" in llm.user
