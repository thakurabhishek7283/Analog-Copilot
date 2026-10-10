"""Verify the assembled circuit and repair a scratch copy, committing only a passing result.

Existing block specifications are immutable across repairs. The model can retune parts, rewire,
or replace/insert library blocks, but cannot lower targets or remove a requirement to pass.
"""

from __future__ import annotations

import asyncio
import json
from dataclasses import dataclass
from typing import Any, Literal

import circuit_core as cc
from pydantic import Field
from sim_runner import client as sim_client
from sim_runner.protocol import KEEP_RESULT_S, SimRefused, SimRequest

from ..jobs.runner import Failure
from ..llm.base import LlmRequest
from ..llm.gateway import LlmUnavailable
from ..llm.prompts import ENV
from . import intent
from .schemas import Named, SchemaError, Strict, parse, provider_schema
from .verifier import failed_checks

MAX_REPAIRS = 2


class SetParam(Strict):
    kind: Literal["set_param"]
    refdes: str
    key: str
    value: str


class Wiring(Strict):
    kind: Literal["connect", "disconnect"]
    net: str
    pins: list[str] = Field(min_length=1, max_length=100)


class Insert(Strict):
    kind: Literal["insert_block"]
    id: str
    template: str
    targets: list[Named]
    ports: list[Named] = Field(description="port name and existing net id as value")


class Remove(Strict):
    kind: Literal["remove_block"]
    id: str


class Repair(Strict):
    explanation: str = Field(max_length=1000)
    unsupported: str = Field(max_length=1000, description="why no supported repair exists, or empty")
    changes: list[SetParam | Wiring | Insert | Remove] = Field(max_length=40)


@dataclass
class Verdict:
    status: str
    checks: list[dict[str, Any]]
    problems: list[str]
    requirements: list[dict[str, Any]] | None = None

    def summary(self, rev: int, attempt: int) -> dict[str, Any]:
        return {"rev": rev, "status": self.status, "attempt": attempt, "max_attempts": MAX_REPAIRS,
                "checks": self.checks, "requirements": self.requirements or [], "problems": self.problems}


async def assess(job, plan, session: cc.Session) -> Verdict:
    verdict = await verify(session, job.ctx.redis, job.ctx.registry_version)
    if verdict.status == "passed":
        requirements, problems, status = await intent.review(job, plan, session, verdict.checks)
        verdict.requirements = requirements
        verdict.problems += problems
        verdict.status = status
    return verdict


async def verify(session: cc.Session, redis, registry_version: str) -> Verdict:
    """No bench sources or stabilizing shunts: measure the actual assembled circuit."""
    issues = cc.unwrap(session.erc("llm_block"))
    # An exposed output can be unloaded; unused IC units are already warnings.
    errors = [e["message"] for e in issues if e["severity"] == "error" and e["code"] != "dangling_net"]
    compiled = json.loads(session.compile(json.dumps({"interactive": True, "transfer_checks": True})))
    if "err" in compiled:
        return Verdict("failed", [], errors + [compiled["err"]["message"]])
    netlist = compiled["ok"]
    missing = [f"{c['block']}: {c['label']}: {c['missing']}" for c in netlist["checks"] if c.get("missing")]
    if missing or not netlist["checks"]:
        return Verdict("incomplete", [], errors + (missing or ["No specification checks are available."]))
    if errors:
        return Verdict("failed", [], errors)
    request = SimRequest(netlist["text"], netlist["includes"], netlist["hash"], registry_version)
    # Retry transport/worker timeouts without asking the model to change a sound circuit.
    for attempt in range(3):
        completed = False
        try:
            result = await sim_client.simulate(redis, request, wait_s=10.0)
            completed = True
        except TimeoutError:
            result = None
        except SimRefused as e:
            if e.code != "internal":
                return Verdict("simulation_error", [], [f"Simulator refused the circuit: {e.code}: {e.message}"])
            result = None
            completed = True
        if result is not None and result.status != "timeout":
            break
        if attempt < 2:
            # arq retains completed results briefly, including worker errors/timeouts.
            # Let that result expire before enqueueing the same netlist again.
            await asyncio.sleep(KEEP_RESULT_S + 0.1 if completed else 0.25 * 2**attempt)
    if result is None or result.status == "timeout":
        return Verdict("simulation_error", [], ["The simulator timed out after three attempts."])
    if result.status != "ok":
        # A reproducible convergence/topology failure can be repaired by the model.
        return Verdict("failed", [], [f"Simulation {result.status}: {result.log[-2000:]}"])
    checks = cc.unwrap(cc.evaluate_checks(json.dumps(netlist["checks"]), json.dumps(result.meas)))
    problems = [f"{c['block']}: {e['message']}" for c in checks for e in failed_checks([c])]
    return Verdict("failed" if problems else "passed", checks, problems)


def trial(session: cc.Session, repair: Repair, job_id: str) -> tuple[cc.Session, list[dict[str, Any]]]:
    """Translate the bounded repair vocabulary into core ops on an independent session."""
    before = json.loads(session.snapshot())
    fork = session.fork()
    ops: list[dict[str, Any]] = []
    for change in repair.changes:
        if isinstance(change, SetParam):
            batch = [{"op": "part.set_param", "body": {"refdes": change.refdes, "key": change.key, "value": change.value}}]
        elif isinstance(change, Wiring):
            batch = [{"op": f"net.{change.kind}", "body": {"net": change.net, "pins": change.pins}}]
        elif isinstance(change, Remove):
            batch = [{"op": "block.set_status", "body": {"id": change.id, "status": "composing"}},
                     {"op": "block.abort", "body": {"id": change.id, "reason": "assembly repair"}}]
        else:
            if len({p.name for p in change.ports}) != len(change.ports) or len({t.name for t in change.targets}) != len(change.targets):
                raise ValueError("each port and target must occur once")
            req = {"id": change.id, "template": change.template,
                   "targets": {t.name: t.value for t in change.targets},
                   "ports": {p.name: {"net": p.value} for p in change.ports}}
            batch = cc.unwrap(fork.insert_block(json.dumps(req)))["ops"]
        for op in batch:
            cc.unwrap(fork.apply(json.dumps({"v": 1, "seq": fork.rev + 1, "base_rev": fork.rev,
                                             "author": "llm", "job": job_id, **op})))
            ops.append(op)
    if not ops:
        raise ValueError("the repair contains no changes")
    after = json.loads(fork.snapshot())
    for bid, block in before["blocks"].items():
        updated = after["blocks"].get(bid)
        if updated is None or updated["spec"] != block["spec"] or updated["role"] != block["role"]:
            raise ValueError(f"repair must preserve {bid} and all its original specification targets")
    # Source frequency is a stimulus, not necessarily a template check. Do not let a model
    # pass by changing the test signal or power rails instead of repairing the circuit.
    for ref, part in before["parts"].items():
        if not ref.startswith("V"):
            continue
        updated = after["parts"].get(ref)
        if updated is None or (updated["part"], updated["params"]) != (part["part"], part["params"]):
            raise ValueError(f"repair must preserve source {ref} and its parameters")
    return fork, ops


async def run(job, plan) -> None:
    """At most two model repairs. Failed candidates never enter the project or event op log."""
    ctx = job.ctx
    ctx.assembly_started = True
    await ctx.set_state("verifying_circuit")
    session = await ctx.session()
    pending = Verdict("incomplete", [], ["Assembly verification has not completed."]).summary(session.rev, 0)
    await ctx.set_plan(plan.to_json() | {"verification": pending, "assembly_attempts": []})
    await ctx.emit("circuit.summary", pending)
    original = await assess(job, plan, session)
    history: list[dict[str, Any]] = []

    async def report(verdict: Verdict, attempt: int) -> None:
        summary = verdict.summary(session.rev, attempt)
        await ctx.set_plan(plan.to_json() | {"verification": summary, "assembly_attempts": history})
        await ctx.emit("circuit.summary", summary)

    await report(original, 0)
    if original.status in ("passed", "incomplete"):
        return
    if original.status == "simulation_error":
        raise Failure("assembly_simulation_failed", original.problems[0], retryable=False)

    verdict = original
    for attempt in range(1, MAX_REPAIRS + 1):
        await ctx.set_state("repairing_circuit")
        await ctx.emit("circuit.summary", original.summary(session.rev, attempt) | {"status": "repairing"})
        prompt = ENV.get_template("assembly.jinja").render(
            prompt=job.prompt, circuit=session.circuit_text(), snapshot=intent.prompt_snapshot(session),
            problems=original.problems, checks=original.checks, history=history,
        )
        try:
            response = await job.gw.complete(LlmRequest(
                "assembly_repair", "small" if job.templates_only else "large", job.prompts.system, prompt,
                schema=provider_schema(Repair), schema_name="assembly_repair", max_tokens=4000,
            ))
            job.usage(response)
            repair = parse(Repair, response.text, response.finish_reason)
            if repair.unsupported:
                history.append({"attempt": attempt, "problems": [repair.unsupported]})
                break
            candidate, ops = trial(session, repair, str(ctx.job_id))
            await ctx.set_state("verifying_circuit")
            verdict = await assess(job, plan, candidate)
            history.append({"attempt": attempt, "repair": repair.model_dump(), "status": verdict.status,
                            "checks": verdict.checks, "problems": verdict.problems})
            if verdict.status == "passed":
                await ctx.commit(ops, author="llm", repair_attempt=attempt, base_rev=session.rev)
                session = candidate
                await job.lesson("repair", repair.explanation or "Repaired the assembled circuit.")
                await report(verdict, attempt)
                return
            if verdict.status == "simulation_error":
                break
        except (SchemaError, cc.OpRejected, ValueError) as e:
            history.append({"attempt": attempt, "problems": [str(e)],
                            "reply": response.text[:8000]})
        except LlmUnavailable as e:
            history.append({"attempt": attempt, "problems": [f"Repair model unavailable: {e}"]})
            break
        # Persist rejected attempts too, so a cancelled job still explains its last trial.
        await report(original, attempt)

    await report(original, attempt)
    reason = "; ".join(history[-1]["problems"][:3]) if history else "No repair passed."
    await job.lesson("note", f"The assembled circuit did not pass verification after {attempt} repair attempt(s). {reason}")
    raise Failure("assembly_verification_failed",
                  f"Circuit kept with unresolved verification failures after {attempt} repair attempt(s). {reason}", False)
