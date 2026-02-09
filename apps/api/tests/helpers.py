"""Small client-side helpers: op envelopes, SSE readers, and wire-model checks."""

from __future__ import annotations

import json
from collections.abc import AsyncIterator
from typing import Any

import httpx
import jwt

from tutor_api.models.contract import AskEvent, JobEvent


def envelopes(ops: list[dict[str, Any]], base_rev: int, author: str = "user") -> list[dict[str, Any]]:
    """Bare ops as consecutive envelopes, the way the editor syncs a batch."""
    return [{"v": 1, "seq": base_rev + i + 1, **op, "author": author, "base_rev": base_rev + i} for i, op in enumerate(ops)]


def user_id(headers: dict[str, str]) -> str:
    token = headers["Authorization"].split()[1]
    return jwt.decode(token, options={"verify_signature": False})["sub"]


async def create_project(http: httpx.AsyncClient, headers: dict[str, str], title: str | None = None) -> str:
    r = await http.post("/v1/projects", json={"title": title} if title else None, headers=headers)
    assert r.status_code == 201, r.text
    return r.json()["id"]


async def sse(r: httpx.Response) -> AsyncIterator[dict[str, Any]]:
    """The events of an SSE response, each `{"id": int | None, "event": str, "data": Any}`."""
    assert r.headers["content-type"].startswith("text/event-stream")
    fields: dict[str, str] = {}
    async for line in r.aiter_lines():
        if line.startswith(":"):
            continue  # keep-alive comment
        if line:
            key, _, value = line.partition(":")
            fields[key] = value[1:] if value.startswith(" ") else value
            continue
        if not fields:
            continue
        yield {
            "id": int(fields["id"]) if "id" in fields else None,
            "event": fields["event"],
            "data": json.loads(fields["data"]) if "data" in fields else None,
        }
        fields = {}


async def read_events(
    http: httpx.AsyncClient, job_id: str, headers: dict[str, str], *, last_event_id: int | None = None,
    stop_at: str | None = None,
) -> list[dict[str, Any]]:
    """Read `/v1/jobs/{id}/events` until the server ends it (or an event named `stop_at`). Each
    event is checked against the wire `JobEvent`."""
    h = dict(headers)
    if last_event_id is not None:
        h["Last-Event-ID"] = str(last_event_id)
    events: list[dict[str, Any]] = []
    async with http.stream("GET", f"/v1/jobs/{job_id}/events", headers=h) as r:
        assert r.status_code == 200, await r.aread()
        async for e in sse(r):
            JobEvent.model_validate({"event": e["event"]} | ({"data": e["data"]} if e["data"] is not None else {}))
            events.append(e)
            if e["event"] == stop_at:
                break
    return events


async def ask(http: httpx.AsyncClient, pid: str, headers: dict[str, str], body: dict[str, Any]) -> list[dict[str, Any]]:
    """`POST /v1/projects/{id}/ask` read to its end; each event is checked against the wire `AskEvent`."""
    async with http.stream("POST", f"/v1/projects/{pid}/ask", headers=headers, json=body) as r:
        assert r.status_code == 200, await r.aread()
        events = [e async for e in sse(r)]
    for e in events:
        AskEvent.model_validate({"event": e["event"], "data": e["data"]})
    return events
