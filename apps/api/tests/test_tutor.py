"""The tutor service without a server (LLD §9, §14): the fixed safety note for mains-powered parts
and the model tier per request kind."""

from __future__ import annotations

import json

import circuit_core as cc
import pytest

from tutor_api.config import REPO
from tutor_api.llm.config import tutor_tiers_from_env
from tutor_api.llm.fake import FakeProvider
from tutor_api.llm.gateway import Gateway
from tutor_api.models.contract import AskRequest
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

    asked = await tutor.answer(s, req, tutor.context(s, req), on_delta)
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


def test_the_ask_tier_is_small_unless_configured():
    assert tutor_tiers_from_env({}) == {"ask": "small"}
    assert tutor_tiers_from_env({"LLM_TIER_ASK": " Large "}) == {"ask": "large"}
    with pytest.raises(RuntimeError, match="LLM_TIER_ASK"):
        tutor_tiers_from_env({"LLM_TIER_ASK": "medium"})
    s = sine_into_rc(registry(mains=False))
    tutor = Tutor(Gateway(FakeProvider({})), {"ask": "large"})
    req = AskRequest.model_validate({"question": "Why?", "rev": s.rev})
    assert tutor.request(req, tutor.context(s, req)).tier == "large"
