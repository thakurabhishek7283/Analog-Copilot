"""The tutor (LLD §9): a question about the circuit at one revision, answered by the model from
circuit-core's context slice, or the edits between two revisions ("What changed?") explained from
circuit-core's change summary; either answer is read back against the circuit (its references and
its `try` block). Mains safety is not left to the model: the server appends a fixed note."""

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
from ..models.contract import AskRequest, ChangeRequest, ReasoningEffort

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


def _wire(req: AskRequest | ChangeRequest) -> str:
    # Wire names (`pass`, not `pass_`); defaults left out: the core has the same ones (and Pydantic
    # warns on the generated str defaults).
    return req.model_dump_json(by_alias=True, exclude_none=True, exclude_defaults=True)


def _outcome(result: str) -> dict[str, Any]:
    """A core `Outcome`'s value; its error is 422 with the core's code."""
    out = json.loads(result)
    if "err" in out:
        raise ApiException(422, out["err"]["code"], out["err"]["message"])
    return out["ok"]


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
        return _outcome(session.tutor_context(_wire(req)))

    @staticmethod
    def changes(before: cc.Session, after: cc.Session, req: ChangeRequest) -> dict[str, Any]:
        """circuit-core's `ChangeContext`: what the edits from `before` to `after` did."""
        return _outcome(after.tutor_changes(before, _wire(req)))

    def request(self, req: AskRequest, context: dict[str, Any]) -> LlmRequest:
        user = prompts.ask(
            level=str(req.level or "beginner"), mode=str(req.mode or "explain"), context=context["text"],
            question=req.question,
        )
        return self._request("ask", user, req.effort)

    def change_request(self, req: ChangeRequest, context: dict[str, Any]) -> LlmRequest:
        user = prompts.what_changed(level=str(req.level or "beginner"), mode=str(req.mode or "explain"),
                                    context=context["text"])
        return self._request("what_changed", user, req.effort)

    def _request(self, kind: str, user: str, effort: ReasoningEffort | None) -> LlmRequest:
        # One system prompt for both kinds, so a provider caches one prefix.
        return LlmRequest(kind, self.tiers[kind], prompts.TUTOR_SYSTEM, user, max_tokens=ASK_MAX_TOKENS,
                          temperature=ASK_TEMPERATURE, effort=str(effort) if effort else None,
                          timeout_s=ASK_CALL_TIMEOUT_S)

    async def answer(self, session: cc.Session, request: LlmRequest, hazards: list[str], on_delta: OnDelta) -> Asked:
        """Stream the model's answer to `on_delta`, then the safety note if `hazards` lists parts,
        and read the whole text against `session`. Raises the gateway's `LlmUnavailable` when no
        provider answers."""
        resp = await self.gateway.stream(request, on_delta)
        text = resp.text
        if hazards:
            note = safety_note(hazards)
            await on_delta(note)
            text += note
        return Asked(text, json.loads(session.read_answer(text)), resp.model, resp.usage)
