"""Review the assembled circuit against the original request after electrical checks pass."""

from __future__ import annotations

import json
from typing import Literal

import circuit_core as cc
from pydantic import Field

from ..llm.base import LlmRequest
from ..llm.gateway import LlmUnavailable
from ..llm.prompts import ENV
from .schemas import SchemaError, Strict, parse, provider_schema


class Requirement(Strict):
    text: str = Field(min_length=3, max_length=300)
    status: Literal["met", "missing", "unverifiable"]
    evidence: str = Field(max_length=500)
    blocks: list[str] = Field(max_length=8)
    checks: list[str] = Field(max_length=30)


class Audit(Strict):
    requirements: list[Requirement] = Field(max_length=30)


def prompt_snapshot(session: cc.Session) -> str:
    """Remove per-run provenance IDs that do not describe the circuit."""
    snapshot = json.loads(session.snapshot())
    for part in snapshot["parts"].values():
        part["origin"].pop("job_id", None)
    return json.dumps(snapshot, separators=(",", ":"))


async def review(job, plan, session: cc.Session, checks: list[dict]) -> tuple[list[dict], list[str], str]:
    """Return requirements, problems, status. A failed audit never becomes a passed claim."""
    prompt = ENV.get_template("assembly_audit.jinja").render(
        prompt=job.prompt, plan=plan.to_json(), circuit=session.circuit_text(),
        snapshot=prompt_snapshot(session), checks=checks,
    )
    try:
        response = await job.gw.complete(LlmRequest(
            "assembly_audit", "small", job.prompts.system, prompt,
            schema=provider_schema(Audit), schema_name="assembly_audit", max_tokens=2500,
        ))
        job.usage(response)
        audit = parse(Audit, response.text, response.finish_reason)
    except (LlmUnavailable, SchemaError) as e:
        return [], [f"Could not review the circuit against the request: {e}"], "incomplete"
    if not audit.requirements:
        return [], ["The request review found no circuit requirements to verify."], "incomplete"

    blocks = json.loads(session.snapshot())["blocks"]
    passed = {f"{c['block']}.{c['name']}" for c in checks if c["pass"]}
    requirements = []
    problems = []
    status = "passed"
    for requirement in audit.requirements:
        r = requirement.model_dump()
        if r["status"] == "met" and (
            not (r["blocks"] or r["checks"])
            or any(b not in blocks for b in r["blocks"])
            or any(c not in passed for c in r["checks"])
        ):
            r["status"] = "unverifiable"
            r["evidence"] = "The cited circuit evidence is missing or did not pass."
        requirements.append(r)
        if r["status"] != "met":
            problems.append(f"Request: {r['text']} — {r['status']}: {r['evidence']}")
            if r["status"] == "missing":
                status = "failed"
            elif status != "failed":
                status = "incomplete"
    return requirements, problems, status
