"""`POST /v1/projects/{id}/ask`, `POST /v1/projects/{id}/what-changed` and
`POST /v1/asks/{id}/feedback` (LLD §5, §9).

Both answers stream as the response to the POST (`answer.delta`, then `answer.done` or `error`),
without a Redis Stream or resume: a dropped stream asks again (decided 2026-02-09). Everything that
can refuse one is checked in its dependency (`prepared_ask`, `prepared_change`), before the stream
starts, so it is a plain HTTP error. The answer is saved to `asks` once it is complete.
"""

from __future__ import annotations

import asyncio
import logging
import time
import uuid
from collections.abc import AsyncIterator
from dataclasses import dataclass
from datetime import UTC, datetime
from typing import Annotated, Any

import circuit_core as cc
from fastapi import APIRouter, Depends, Request, Response
from fastapi.sse import EventSourceResponse, ServerSentEvent
from sqlalchemy import insert, select, update
from sqlalchemy.ext.asyncio import AsyncConnection

from .. import projects
from ..auth import User
from ..db.tables import asks
from ..errors import ApiException, not_found
from ..llm.base import LlmRequest
from ..llm.gateway import LlmUnavailable
from ..models.contract import AskFeedback, AskRequest, ChangeRequest
from ..tutor import MAX_HISTORY, Asked, Turn, Tutor

log = logging.getLogger("tutor_api.ask")

router = APIRouter(prefix="/v1", tags=["tutor"])

MAX_QUESTION = 1000
# What an explained edit is saved as, in `asks.question`.
WHAT_CHANGED = "What changed?"


@dataclass
class Prepared:
    tutor: Tutor
    project_id: uuid.UUID
    user: uuid.UUID
    session: cc.Session  # the circuit the answer is read against, at `rev`
    llm: LlmRequest
    hazards: list[str]
    row: dict[str, Any]  # the rest of its `asks` row: kind, rev, from_rev, selection, question, mode, level, effort, context, history


def available(request: Request) -> Tutor:
    tutor: Tutor | None = request.app.state.tutor
    if tutor is None:
        raise ApiException(503, "tutor_unavailable", "no tutor model is configured on this server")
    return tutor


def synced(rev: int, head_rev: int) -> None:
    if rev > head_rev:
        # The editor has edits the server has not stored yet: it asks again once they sync.
        raise ApiException(409, "rev_not_synced", f"rev {rev} but the project is at rev {head_rev}", retryable=True)


def history_ids(ids: list[str]) -> list[uuid.UUID]:
    if len(ids) > MAX_HISTORY or len(set(ids)) != len(ids):
        raise ApiException(422, "invalid_request", f"history holds at most {MAX_HISTORY} different answers")
    try:
        return [uuid.UUID(i) for i in ids]
    except ValueError:
        raise ApiException(422, "invalid_request", "history holds answer ids") from None


async def conversation(conn: AsyncConnection, ids: list[uuid.UUID], project: uuid.UUID, user: uuid.UUID,
                       rev: int) -> list[Turn]:
    """The earlier answers a question follows, in the order given: the learner's own, in this
    project. One that is not is 422 (the editor sends only answers it was given here)."""
    if not ids:
        return []
    found = {r.id: r for r in await conn.execute(
        select(asks.c.id, asks.c.question, asks.c.answer, asks.c.rev)
        .where(asks.c.id.in_(ids), asks.c.project_id == project, asks.c.user_id == user)
    )}
    if missing := [str(i) for i in ids if i not in found]:
        raise ApiException(422, "invalid_request", f"history: no answer {', '.join(missing)} in this project")
    return [Turn.of(found[i].question, found[i].answer, found[i].rev < rev) for i in ids]


async def prepared_ask(project_id: str, body: AskRequest, request: Request, user: User) -> Prepared:
    st = request.app.state
    tutor = available(request)
    question = body.question.strip()
    if not question or len(question) > MAX_QUESTION:
        raise ApiException(422, "invalid_request", f"question must be 1 to {MAX_QUESTION} characters")
    ids = history_ids(body.history or [])
    async with st.engine.connect() as conn:
        row = await projects.get_owned(conn, project_id, user)
        synced(body.rev, row.head_rev)
        session = await projects.load_session(conn, st.regs, row, at=body.rev)
        turns = await conversation(conn, ids, row.id, user, body.rev)
    req = body.model_copy(update={"question": question})
    context = tutor.context(session, req)
    saved = {"kind": "ask", "rev": req.rev, "from_rev": None, "question": question, "mode": str(req.mode or "explain"),
             "selection": req.selection.model_dump(mode="json") if req.selection else None,
             "history": ids or None, **details(req, context)}
    return Prepared(tutor, row.id, user, session, tutor.request(req, context, turns), context["hazards"], saved)


async def prepared_change(project_id: str, body: ChangeRequest, request: Request, user: User) -> Prepared:
    st = request.app.state
    tutor = available(request)
    if body.from_rev >= body.rev:
        raise ApiException(422, "invalid_request", f"from_rev ({body.from_rev}) must be before rev ({body.rev})")
    async with st.engine.connect() as conn:
        row = await projects.get_owned(conn, project_id, user)
        synced(body.rev, row.head_rev)
        before = await projects.load_session(conn, st.regs, row, at=body.from_rev)
        after = await projects.load_session(conn, st.regs, row, at=body.rev)
    context = tutor.changes(before, after, body)
    saved = {"kind": "what_changed", "rev": body.rev, "from_rev": body.from_rev, "question": WHAT_CHANGED,
             "mode": str(body.mode or "explain"), "selection": None, **details(body, context)}
    return Prepared(tutor, row.id, user, after, tutor.change_request(body, context), context["hazards"], saved)


def details(req: AskRequest | ChangeRequest, context: dict[str, Any]) -> dict[str, Any]:
    """What the learner chose and what the model was given, kept with the answer (LLD §11)."""
    return {"level": str(req.level or "beginner"), "effort": str(req.effort) if req.effort else None,
            "context": context["text"]}


def error(code: str, message: str, retryable: bool) -> ServerSentEvent:
    return ServerSentEvent(event="error", data={"code": code, "message": message, "retryable": retryable})


async def answered(p: Prepared, request: Request) -> AsyncIterator[ServerSentEvent]:
    """The model's answer as `answer.delta` events, saved, then `answer.done`; or one `error`."""
    st = request.app.state
    deltas: asyncio.Queue[str | None] = asyncio.Queue()
    started = time.perf_counter()
    first: float | None = None

    async def answer() -> Asked:
        async with asyncio.timeout(st.settings.ask_timeout_s):
            return await p.tutor.answer(p.session, p.llm, p.hazards, deltas.put)

    work = asyncio.create_task(answer())
    work.add_done_callback(lambda _: deltas.put_nowait(None))
    try:
        while (text := await deltas.get()) is not None:
            first = first or time.perf_counter()
            yield ServerSentEvent(event="answer.delta", data={"text": text})
        asked = work.result()
    except TimeoutError:
        yield error("tutor_timeout", f"no answer within {st.settings.ask_timeout_s:g} s", True)
        return
    except LlmUnavailable as e:
        yield error("llm_unavailable", f"The model is not answering: {e}", True)
        return
    except Exception:
        log.exception("%s on project %s failed", p.llm.kind, p.project_id)
        yield error("internal", "the tutor failed", False)
        return
    finally:
        work.cancel()  # a client that disconnects cancels this generator: stop the model call with it

    ask_id = uuid.uuid4()
    answer = asked.answer
    elapsed = {"first_token_ms": round((first - started) * 1e3) if first else None,
               "ms": round((time.perf_counter() - started) * 1e3)}
    async with st.engine.begin() as conn:
        await conn.execute(insert(asks).values(
            id=ask_id, project_id=p.project_id, user_id=p.user, answer=asked.text,
            refs_valid=answer["refs_valid"], refs_invalid=answer["refs_invalid"], model=asked.model,
            in_tokens=asked.usage.in_tokens, out_tokens=asked.usage.out_tokens, created_at=datetime.now(UTC),
            **p.row, **elapsed,
        ))
    usage = {"in_tokens": asked.usage.in_tokens, "out_tokens": asked.usage.out_tokens}
    yield ServerSentEvent(event="answer.done", data={"ask_id": str(ask_id), "answer": answer, "usage": usage})


@router.post("/projects/{project_id}/ask", response_class=EventSourceResponse)
async def ask(p: Annotated[Prepared, Depends(prepared_ask)], request: Request) -> AsyncIterator[ServerSentEvent]:
    async for event in answered(p, request):
        yield event


@router.post("/projects/{project_id}/what-changed", response_class=EventSourceResponse)
async def what_changed(p: Annotated[Prepared, Depends(prepared_change)], request: Request) -> AsyncIterator[ServerSentEvent]:
    async for event in answered(p, request):
        yield event


@router.post("/asks/{ask_id}/feedback", status_code=204)
async def feedback(ask_id: str, body: AskFeedback, request: Request, user: User) -> Response:
    if body.feedback not in (1, -1):
        raise ApiException(422, "invalid_request", "feedback must be 1 (helpful) or -1 (not helpful)")
    try:
        aid = uuid.UUID(ask_id)
    except ValueError:
        raise not_found("answer") from None
    async with request.app.state.engine.begin() as conn:
        done = await conn.execute(
            update(asks).where(asks.c.id == aid, asks.c.user_id == user).values(feedback=body.feedback).returning(asks.c.id)
        )
        if done.one_or_none() is None:
            raise not_found("answer")
    return Response(status_code=204)
