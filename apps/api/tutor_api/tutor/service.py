"""The tutor (LLD §9): a question about the circuit at one revision, answered by the model from
circuit-core's context slice, then read back against that circuit (its references and its `try`
block). Mains safety is not left to the model: the server appends a fixed note."""

from __future__ import annotations

import json
from collections.abc import Mapping
from dataclasses import dataclass, field
from typing import Any

import circuit_core as cc

from ..errors import ApiException
from ..llm import prompts
from ..llm.base import LlmRequest, OnDelta, Tier, Usage
from ..llm.config import TUTOR_TIERS
from ..llm.gateway import Gateway
from ..models.contract import AskRequest

ASK_MAX_TOKENS = 4000  # about 150 words and a `try` block; the rest is room for a reasoning model's thinking at high effort
ASK_TEMPERATURE = 0.3
# Latency is not a goal for Ask (decided 2026-02-09): a reasoning model at high effort sends nothing
# while it thinks (14 s live), so its calls get far more than the 30 s generation calls get.
ASK_CALL_TIMEOUT_S = 120.0


def safety_note(hazards: list[str]) -> str:
    """The fixed note for a circuit with `hazard: mains` parts (LLD §14), citing them."""
    parts = ", ".join(f"[{r}]" for r in hazards)
    return (
        f"\n\nSafety: {parts} {'runs' if len(hazards) == 1 else 'run'} from mains voltage, which can kill. "
        "Explore this circuit here in simulation; build or probe it only with a qualified supervisor "
        "and an isolation transformer."
    )


@dataclass
class Asked:
    text: str  # as streamed: the model's reply, then any safety note
    answer: dict[str, Any]  # circuit-core's `Answer`
    model: str
    usage: Usage


@dataclass
class Tutor:
    gateway: Gateway
    tiers: Mapping[str, Tier] = field(default_factory=lambda: dict(TUTOR_TIERS))

    @staticmethod
    def context(session: cc.Session, req: AskRequest) -> dict[str, Any]:
        """circuit-core's `TutorContext`. A selection the circuit lacks is 422 with the core's code
        (`part_not_found`, `net_not_found`, `block_not_found`)."""
        # Wire names (`pass`, not `pass_`); defaults left out: the core has the same ones (and
        # Pydantic warns on the generated str defaults).
        wire = req.model_dump_json(by_alias=True, exclude_none=True, exclude_defaults=True)
        out = json.loads(session.tutor_context(wire))
        if "err" in out:
            raise ApiException(422, out["err"]["code"], out["err"]["message"])
        return out["ok"]

    def request(self, req: AskRequest, context: dict[str, Any]) -> LlmRequest:
        user = prompts.ask(
            level=str(req.level or "beginner"), mode=str(req.mode or "explain"), context=context["text"],
            question=req.question,
        )
        return LlmRequest("ask", self.tiers["ask"], prompts.TUTOR_SYSTEM, user, max_tokens=ASK_MAX_TOKENS,
                          temperature=ASK_TEMPERATURE, effort=str(req.effort) if req.effort else None,
                          timeout_s=ASK_CALL_TIMEOUT_S)

    async def answer(self, session: cc.Session, req: AskRequest, context: dict[str, Any], on_delta: OnDelta) -> Asked:
        """Stream the model's answer to `on_delta`, then the safety note if one is due. Raises the
        gateway's `LlmUnavailable` when no provider answers."""
        resp = await self.gateway.stream(self.request(req, context), on_delta)
        text = resp.text
        if context["hazards"]:
            note = safety_note(context["hazards"])
            await on_delta(note)
            text += note
        return Asked(text, json.loads(session.read_answer(text)), resp.model, resp.usage)
