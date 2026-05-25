"""The session export on a real database: a student asks, applies the suggested experiment, asks
"What changed?" and rates an answer; the export finds each answer with its checks."""

from __future__ import annotations

import asyncio
import json
import secrets
import sys
from pathlib import Path

import httpx

import fixtures as fx
import stack
from tutor_api.config import Settings
from tutor_api.llm.fake import FakeProvider
from tutor_api.llm.gateway import Gateway
from tutor_api.main import create_app
from tutor_api.orchestrator import Orchestrator
from tutor_api.tutor import Tutor

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "sessions"))
import export  # noqa: E402

TRY = {"ops": [{"op": "part.set_param", "body": {"refdes": "R1", "key": "resistance", "value": "36k"}}],
       "predict": "fc drops to about 700 Hz"}
SCRIPT = {"rules": [
    {"kind": "ask", "when": [], "reply": f"[R1] (18 kΩ) sets fc with [C1]; it is 42 kHz.\n\n```try\n{json.dumps(TRY)}\n```"},
    {"kind": "what_changed", "when": [], "reply": "[R1] went up, so fc fell; see [R7]."},
]}


async def a_session(database_url: str, redis_url: str) -> None:
    gw = Gateway(FakeProvider(SCRIPT))
    app = create_app(Settings(database_url=database_url, redis_url=redis_url, auth_secret=secrets.token_urlsafe(32)),
                     Orchestrator(gw), Tutor(gw))
    circuit = json.loads(fx.FIXTURES.read_text(encoding="utf-8"))["circuits"]["sk_lp"]
    async with stack.api(app) as (url, _), httpx.AsyncClient(base_url=url) as http:
        h = {"Authorization": f"Bearer {(await http.post('/v1/auth/anonymous')).json()['token']}"}
        pid = (await http.post("/v1/projects", json={}, headers=h)).json()["id"]
        r = await http.post(f"/v1/projects/{pid}/ops", json={"base_rev": 0, "ops": fx.envelopes(circuit["batches"])}, headers=h)
        rev = r.json()["rev"]
        body = {"question": "Why is R1 18k?", "rev": rev, "sim": circuit["sim"], "level": "intermediate", "effort": "low"}
        async with http.stream("POST", f"/v1/projects/{pid}/ask", json=body, headers=h) as resp:
            done = [json.loads(line[6:]) for line in [x async for x in resp.aiter_lines()] if line.startswith("data: ")][-1]
        await http.post(f"/v1/asks/{done['ask_id']}/feedback", json={"feedback": -1}, headers=h)
        follow = {**body, "question": "And C1?", "history": [done["ask_id"]]}
        async with http.stream("POST", f"/v1/projects/{pid}/ask", json=follow, headers=h) as resp:
            await resp.aread()
        # Try it: the suggestion as one user batch
        r = await http.post(f"/v1/projects/{pid}/ops", json={"base_rev": rev, "ops": fx.envelopes([{"author": "user", "ops": TRY["ops"]}], rev)},
                            headers=h)
        after = r.json()["rev"]
        change = {"from_rev": rev, "rev": after, "before": circuit["sim"], "after": circuit["sim"]}
        async with http.stream("POST", f"/v1/projects/{pid}/what-changed", json=change, headers=h) as resp:
            await resp.aread()


def test_the_export_reads_each_answer_with_its_checks(databases, tmp_path):
    asyncio.run(a_session(*databases))
    rows, log = asyncio.run(export.load(databases[0], None, None))
    out = export.answers(rows, log, fx.registry())
    mine = [r for r in out if r["question"] in ("Why is R1 18k?", "What changed?")][-2:]
    ask, changed = mine
    assert (ask["kind"], ask["level"], ask["effort"], ask["feedback"]) == ("ask", "intermediate", "low", "no")
    assert (ask["refs_valid"], ask["refs_invalid"]) == (2, 0)
    assert ask["ungrounded"] == "42 kHz (not in the context)" and ask["quantities"] == 2
    assert (ask["try"], ask["try_applied"], ask["try_predict"]) == ("yes", "yes", TRY["predict"])
    assert (changed["kind"], changed["refs_invalid"], changed["try"], changed["effort"]) == ("what_changed", 1, "", "normal")
    assert (ask["invalid_refs"], changed["invalid_refs"]) == ("", "[R7]"), "named from the circuit at the answer's rev"
    assert ask["session"] == changed["session"] and int(ask["ms"]) >= int(ask["first_token_ms"]) >= 0
    (follow,) = [r for r in out if r["question"] == "And C1?"][-1:]
    assert (ask["follows"], follow["follows"]) == ("", "“Why is R1 18k?”")

    md = export.markdown(mine, "test")
    assert "| **All** | 2 | 1 | 0 | 75.0% (4) | 50.0% | 1/1 | 0.0% (1) |" in md
    assert "## Invalid references (1)" in md and "## Answers marked not helpful (1)" in md
    assert "(what_changed): “What changed?” — [R7]" in md
    assert export.main(["--database-url", databases[0], "--out", str(tmp_path)]) == 0
    assert (tmp_path / "answers.csv").read_text(encoding="utf-8-sig").startswith("session,time,project,kind")
