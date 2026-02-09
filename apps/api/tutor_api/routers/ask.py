"""`POST /v1/projects/{id}/ask` and `POST /v1/asks/{id}/feedback` (LLD §5, §9).

The answer streams as the response to the POST (`answer.delta`, then `answer.done` or `error`),
without a Redis Stream or resume: a dropped stream asks again (decided 2026-02-09). Everything that
can refuse a question is checked in `prepared`, before the stream starts, so it is a plain HTTP
error. The answer is saved to `asks` once it is complete.
"""

from __future__ import annotations

import asyncio
import logging
import uuid
from collections.abc import AsyncIterator
from dataclasses import dataclass
from datetime import UTC, datetime
from typing import Annotated, Any

import circuit_core as cc
from fastapi import APIRouter, Depends, Request, Response
from fastapi.sse import EventSourceResponse, ServerSentEvent
from sqlalchemy import insert, update

from .. import projects
from ..auth import User
from ..db.tables import asks
from ..errors import ApiException, not_found
from ..llm.gateway import LlmUnavailable
from ..models.contract import AskFeedback, AskRequest
from ..tutor import Asked, Tutor

log = logging.getLogger("tutor_api.ask")

router = APIRouter(prefix="/v1", tags=["tutor"])

MAX_QUESTION = 1000


@dataclass
class Prepared:
    tutor: Tutor
    project_id: uuid.UUID
    user: uuid.UUID
    session: cc.Session
    request: AskRequest
    context: dict[str, Any]


async def prepared(project_id: str, body: AskRequest, request: Request, user: User) -> Prepared:
    st = request.app.state
    tutor: Tutor | None = st.tutor
    if tutor is None:
        raise ApiException(503, "tutor_unavailable", "no tutor model is configured on this server")
    question = body.question.strip()
    if not question or len(question) > MAX_QUESTION:
        raise ApiException(422, "invalid_request", f"question must be 1 to {MAX_QUESTION} characters")
    async with st.engine.connect() as conn:
        row = await projects.get_owned(conn, project_id, user)
        if body.rev > row.head_rev:
            # The editor has edits the server has not stored yet: it asks again once they sync.
            raise ApiException(409, "rev_not_synced", f"rev {body.rev} but the project is at rev {row.head_rev}",
                               retryable=True)
        session = await projects.load_session(conn, st.regs, row, at=body.rev)
    req = body.model_copy(update={"question": question})
    return Prepared(tutor, row.id, user, session, req, tutor.context(session, req))


def error(code: str, message: str, retryable: bool) -> ServerSentEvent:
    return ServerSentEvent(event="error", data={"code": code, "message": message, "retryable": retryable})


@router.post("/projects/{project_id}/ask", response_class=EventSourceResponse)
async def ask(p: Annotated[Prepared, Depends(prepared)], request: Request) -> AsyncIterator[ServerSentEvent]:
    st = request.app.state
    deltas: asyncio.Queue[str | None] = asyncio.Queue()

    async def answer() -> Asked:
        async with asyncio.timeout(st.settings.ask_timeout_s):
            return await p.tutor.answer(p.session, p.request, p.context, deltas.put)

    work = asyncio.create_task(answer())
    work.add_done_callback(lambda _: deltas.put_nowait(None))
    try:
        while (text := await deltas.get()) is not None:
            yield ServerSentEvent(event="answer.delta", data={"text": text})
        asked = work.result()
    except TimeoutError:
        yield error("tutor_timeout", f"no answer within {st.settings.ask_timeout_s:g} s", True)
        return
    except LlmUnavailable as e:
        yield error("llm_unavailable", f"The model is not answering: {e}", True)
        return
    except Exception:
        log.exception("ask on project %s failed", p.project_id)
        yield error("internal", "the tutor failed", False)
        return
    finally:
        work.cancel()  # a client that disconnects cancels this generator: stop the model call with it

    ask_id = uuid.uuid4()
    answer = asked.answer
    selection = p.request.selection.model_dump(mode="json") if p.request.selection else None
    async with st.engine.begin() as conn:
        await conn.execute(insert(asks).values(
            id=ask_id, project_id=p.project_id, user_id=p.user, rev=p.request.rev, selection=selection,
            question=p.request.question, answer=asked.text, mode=str(p.request.mode or "explain"),
            refs_valid=answer["refs_valid"], refs_invalid=answer["refs_invalid"], model=asked.model,
            in_tokens=asked.usage.in_tokens, out_tokens=asked.usage.out_tokens, created_at=datetime.now(UTC),
        ))
    usage = {"in_tokens": asked.usage.in_tokens, "out_tokens": asked.usage.out_tokens}
    yield ServerSentEvent(event="answer.done", data={"ask_id": str(ask_id), "answer": answer, "usage": usage})


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
