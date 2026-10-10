"""A separate, revision-bound build guide for verified projects."""

from __future__ import annotations

import json
from typing import Annotated

from fastapi import APIRouter, Body, Request
from pydantic import Field
from sqlalchemy import select

from .. import breadboard as bb, projects
from ..auth import User
from ..errors import ApiException
from ..llm.base import LlmRequest
from ..llm.gateway import LlmUnavailable
from ..db.tables import jobs
from ..orchestrator.schemas import SchemaError, Strict, parse, provider_schema

router = APIRouter(prefix="/v1/projects", tags=["breadboard"])


class Revision(Strict):
    rev: int = Field(ge=0)


class Validate(Strict):
    layout: bb.Layout


async def verified(request: Request, user: User, project_id: str, rev: int) -> tuple[dict, dict, dict, str]:
    st = request.app.state
    async with st.engine.connect() as conn:
        row = await projects.get_owned(conn, project_id, user)
        if row.head_rev != rev:
            raise ApiException(409, "stale_rev", "The finalized circuit has changed; verify it again.")
        if await projects.active_job(conn, row.id):
            raise ApiException(409, "job_running", "Wait for circuit generation to finish.")
        verdict = await projects.verification(conn, row.id, rev)
        if not verdict or verdict.get("status") != "passed":
            raise ApiException(409, "not_verified", "Complete circuit and request verification before creating a build guide.")
        session = await projects.load_session(conn, st.regs, row)
        prompt = (await conn.execute(select(jobs.c.prompt).where(jobs.c.project_id == row.id)
                                     .order_by(jobs.c.started_at.desc()).limit(1))).scalar_one()
    return json.loads(session.snapshot()), json.loads(st.regs.get(row.registry_version).to_json()), verdict, prompt


@router.post("/{project_id}/breadboard/generate")
async def generate(project_id: str, body: Revision, request: Request, user: User) -> dict:
    circuit, registry, verdict, prompt = await verified(request, user, project_id, body.rev)
    unsupported = bb.unsupported(circuit)
    if unsupported:
        raise ApiException(422, "unsupported_footprint", "No supported through-hole footprint for: " + ", ".join(unsupported))
    gateway = request.app.state.breadboard_gateway
    if gateway is None:
        raise ApiException(503, "llm_unavailable", "Configure the AI provider to create a physical build guide.", retryable=True)
    parts = [{"refdes": ref, "part": part["part"], "params": part["params"],
              "pin_order": bb._pins(registry, part["part"]),
              "package": bb.FOOTPRINTS[part["part"]][0], "lead_offsets": bb.FOOTPRINTS[part["part"]][1],
              "rows": max(offset for _, offset in bb.FOOTPRINTS[part["part"]][1]) + 1}
             for ref, part in circuit["parts"].items() if not part["part"].startswith("vsource_")]
    context = {"request": prompt, "verification": {"status": verdict["status"], "checks": verdict.get("checks", []),
               "requirements": verdict.get("requirements", [])}, "rev": body.rev, "parts": parts,
               "nets": {name: net["pins"] for name, net in circuit["nets"].items()},
               "external_sources": [ref for ref, part in circuit["parts"].items() if part["part"].startswith("vsource_")]}
    feedback = ""
    for _ in range(3):
        try:
            response = await gateway.complete(LlmRequest(
                kind="breadboard_plan", tier="large",
                system="Place a verified circuit on up to eight standard 63-row solderless breadboards. Return only JSON matching the schema. Each physical part needs one board number and starting row. Use as few boards as fit while keeping connected parts nearby. Reserve every row covered by its package plus at least one blank row between bodies; do not overlap parts. External sources have no board placement. The server maps actual numbered leads to holes, routes jumpers, and validates all electrical nets. Do not invent or omit parts.",
                user=json.dumps(context, separators=(",", ":")) + feedback,
                schema=provider_schema(bb.Candidate), schema_name="breadboard_plan", max_tokens=8000,
            ))
            candidate = parse(bb.Candidate, response.text, response.finish_reason)
            layout = bb.assemble(candidate, circuit, registry)
            # A model call may outlive a concurrent circuit edit; never approve stale geometry.
            await verified(request, user, project_id, body.rev)
            return {"layout": layout.model_dump(mode="json", by_alias=True), "valid": True,
                    "notes": ["Board rails are not used; only labelled A–J tie strips are assumed.",
                              "Check each purchased part against the listed package and pin orientation before insertion."]}
        except (SchemaError, ValueError, KeyError) as exc:
            feedback = "\nPrevious placement was rejected: " + str(exc) + ". Return a corrected complete placement."
        except LlmUnavailable as exc:
            raise ApiException(503, "llm_unavailable", str(exc), retryable=True) from exc
    raise ApiException(422, "placement_failed", "AI could not produce a physically and electrically valid placement." + feedback)


@router.post("/{project_id}/breadboard/validate")
async def validate(project_id: str, body: Annotated[Validate, Body()], request: Request, user: User) -> dict:
    circuit, registry, _, _ = await verified(request, user, project_id, body.layout.rev)
    errors = bb.validate(body.layout, circuit, registry)
    return {"valid": not errors, "problems": errors}
