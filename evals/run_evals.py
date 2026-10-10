"""Generation evals (LLD §15; the Phase 2 gate, LLD §16): every prompt in evals/prompts.yaml runs as a
real generation job through the real API, on Postgres and Redis in testcontainers with the sim_runner
worker on the native ngspice (tools/e2e/stack.py), and is read back from its SSE stream and its job
row. The metrics, the gate and the report are in metrics.py.

    python evals/run_evals.py --live --record          # the provider in LLM_PROVIDER: real calls, recorded
    python evals/run_evals.py --replay evals/golden/cassette.json --gate --baseline evals/golden/report.json
    python evals/run_evals.py --fake SCRIPT.json       # scripted replies (LLM_PROVIDER=fake's format)
    python evals/run_evals.py --exchange DIR --label "gemini-3.5-flash, AI Studio chat"   # replies from a chat (exchange.py)

A real provider (`gemini`, `deepseek`, `openai`; keys and models as for the API, see
apps/api/tutor_api/llm/config.py) is used only with `--live`. Each run writes report.json,
report.md and, with `--record`, cassette.json to evals/results/<time>-<provider>/. Exit status 1
when `--gate` fails.
"""

from __future__ import annotations

import argparse
import asyncio
import json
import os
import random
import secrets
import subprocess
import sys
import time
from collections.abc import Callable
from dataclasses import dataclass
from datetime import UTC, datetime
from pathlib import Path
from typing import Any

import httpx
import yaml

HERE = Path(__file__).resolve().parent
REPO = HERE.parent
sys.path.insert(0, str(REPO / "tools" / "e2e"))

import stack  # noqa: E402
from exchange import Exchange  # noqa: E402
from metrics import gate, markdown, summarize  # noqa: E402
from sqlalchemy import select  # noqa: E402
from tutor_api.config import Settings  # noqa: E402
from tutor_api.db.tables import block_attempts, jobs  # noqa: E402
from tutor_api.llm.base import LlmRequest, LlmResponse, OnDelta, Provider, ProviderError  # noqa: E402
from tutor_api.llm.cassette import Recorder, Replayer  # noqa: E402
from tutor_api.llm.config import REAL, gateway_from_env  # noqa: E402
from tutor_api.llm.fake import FakeProvider  # noqa: E402
from tutor_api.llm.gateway import Gateway  # noqa: E402
from tutor_api.main import create_app  # noqa: E402
from tutor_api.orchestrator import Orchestrator  # noqa: E402

LEVELS = ("beginner", "intermediate", "advanced")


# ---------------------------------------------------------------- prompts

def load_cases(path: Path, *, only: list[str] | None = None, tags: list[str] | None = None,
               limit: int | None = None) -> list[dict[str, Any]]:
    cases = yaml.safe_load(path.read_text(encoding="utf-8"))
    if only:
        missing = set(only) - {c["id"] for c in cases}
        if missing:
            raise SystemExit(f"no prompt {', '.join(sorted(missing))} in {path}")
        cases = [c for c in cases if c["id"] in only]
    if tags:
        cases = [c for c in cases if set(tags) & set(c.get("tags") or ())]
    return cases[:limit] if limit else cases


# ---------------------------------------------------------------- providers

class Metered:
    """Records every call a provider answers or fails: kind, model, the provider's token counts
    (cached included, and whether they were estimated) and latency. Anything else a call raises is
    recorded by its type, so a cassette miss or an unanswered exchange call shows even where the
    orchestrator only logs it (narration)."""

    def __init__(self, inner: Provider, calls: list[dict[str, Any]]):
        self.inner = inner
        self.name = inner.name
        self.calls = calls

    async def _call(self, req: LlmRequest, call: Callable[[], Any]) -> LlmResponse:
        started = time.perf_counter()
        entry: dict[str, Any] = {"kind": req.kind, "tier": req.tier, "provider": self.name}
        try:
            resp = await call()
        except Exception as e:
            code = e.code if isinstance(e, ProviderError) else type(e).__name__
            self.calls.append(entry | {"error": code, "ms": ms(started), "in_tokens": 0, "out_tokens": 0,
                                       "cached_tokens": 0})
            raise
        self.calls.append(entry | {"model": resp.model, "ms": ms(started), "in_tokens": resp.usage.in_tokens,
                                   "out_tokens": resp.usage.out_tokens, "cached_tokens": resp.usage.cached_tokens,
                                   "estimated": resp.usage.estimated, "finish_reason": resp.finish_reason})
        return resp

    async def complete(self, req: LlmRequest) -> LlmResponse:
        return await self._call(req, lambda: self.inner.complete(req))

    async def stream(self, req: LlmRequest, on_delta: OnDelta) -> LlmResponse:
        return await self._call(req, lambda: self.inner.stream(req, on_delta))


class Paced:
    """Waits out a provider's rate limits (HTTP 429, `rate_limited`) instead of failing the call: an
    eval measures the model, not the account's quota. It waits the delay the provider suggests (else
    30 s), at most `max_wait_s` per call, and never for a daily quota (`quota_exhausted`). The gateway
    and a recording of the run see only what the model did; the waits are counted for the report."""

    def __init__(self, inner: Provider, max_wait_s: float):
        self.inner = inner
        self.name = inner.name
        self.max_wait_s = max_wait_s
        self.waits = 0
        self.waited_s = 0.0

    async def _call(self, call: Callable[[], Any], started: Callable[[], bool]) -> LlmResponse:
        waited = 0.0
        while True:
            try:
                return await call()
            except ProviderError as e:
                if e.code != "rate_limited" or waited >= self.max_wait_s or started():
                    raise
                delay = min(max(e.retry_after_s or 30.0, 1.0) + random.uniform(0, 2), 120.0)
                self.waits += 1
                self.waited_s += delay
                waited += delay
                await asyncio.sleep(delay)

    async def complete(self, req: LlmRequest) -> LlmResponse:
        return await self._call(lambda: self.inner.complete(req), lambda: False)

    async def stream(self, req: LlmRequest, on_delta: OnDelta) -> LlmResponse:
        streamed = False  # a stream that has sent text is never repeated (the gateway's rule too)

        async def tee(text: str) -> None:
            nonlocal streamed
            streamed = True
            await on_delta(text)

        return await self._call(lambda: self.inner.stream(req, tee), lambda: streamed)


def ms(started: float) -> int:
    return round((time.perf_counter() - started) * 1e3)


def describe(p: Provider) -> str:
    models = getattr(p, "models", None)
    return f"{p.name} ({models['large']} / {models['small']})" if models else p.name


@dataclass
class Models:
    gateway: Gateway
    calls: list[dict[str, Any]]
    recorders: list[Recorder]
    description: str
    name: str
    pacers: list[Paced]

    def rate_limit_waits(self) -> dict[str, Any]:
        return {"waits": sum(p.waits for p in self.pacers), "seconds": round(sum(p.waited_s for p in self.pacers), 1)}

    def save_cassette(self, path: Path) -> None:
        self.recorders[0].save(path)


def models(providers: list[Provider], *, record: bool, label: str | None = None, max_wait_s: float = 0) -> Models:
    """The gateway over `providers` (primary, then fallback). Each provider's calls are metered (every
    attempt, rate-limited ones included); with `max_wait_s`, rate limits are waited out; with
    `record`, what the model answered goes into one cassette, in call order."""
    calls: list[dict[str, Any]] = []
    recorders: list[Recorder] = []
    pacers: list[Paced] = []
    wrapped: list[Provider] = []
    for p in providers:
        p = Metered(p, calls)
        if max_wait_s:
            p = Paced(p, max_wait_s)
            pacers.append(p)
        if record:
            rec = Recorder(p)
            if recorders:
                rec.interactions = recorders[0].interactions  # one cassette, in call order
            recorders.append(rec)
            p = rec
        wrapped.append(p)
    description = label or ", then ".join(describe(p) for p in providers)
    return Models(Gateway(*wrapped), calls, recorders, description, providers[0].name, pacers)


def providers_from_args(args: argparse.Namespace) -> tuple[list[Provider], str]:
    if args.replay:
        return [Replayer.from_file(args.replay)], f"replay ({args.replay})"
    if args.fake:
        return [FakeProvider.from_file(args.fake)], f"fake ({args.fake})"
    if args.exchange:
        return [Exchange(args.exchange, args.label)], f"exchange: {args.label} ({args.exchange})"
    name = (os.environ.get("LLM_PROVIDER") or "").strip().lower()
    if not name:
        raise SystemExit("no model: pass --replay CASSETTE or --fake SCRIPT, or set LLM_PROVIDER (with --live)")
    real = name in REAL or (os.environ.get("LLM_FALLBACK_PROVIDER") or "").strip().lower() in REAL
    if real and not args.live:
        raise SystemExit(f"LLM_PROVIDER={name} makes real model calls: pass --live to run them")
    gw = gateway_from_env()
    return gw.providers, ", then ".join(describe(p) for p in gw.providers)


def load_env_file(path: Path) -> None:
    """KEY=VALUE lines into the environment, without overriding what is already set."""
    for line in path.read_text(encoding="utf-8").splitlines():
        line = line.strip()
        if line and not line.startswith("#") and "=" in line:
            key, _, value = line.partition("=")
            os.environ.setdefault(key.strip(), value.strip().strip('"').strip("'"))


# ---------------------------------------------------------------- running

async def read_events(http: httpx.AsyncClient, job_id: str, headers: dict[str, str],
                      t0: float) -> list[tuple[float, str, Any]]:
    """The job's events until `done` or `error`: (seconds since t0, event, data)."""
    out: list[tuple[float, str, Any]] = []
    async with http.stream("GET", f"/v1/jobs/{job_id}/events", headers=headers) as r:
        r.raise_for_status()
        fields: dict[str, str] = {}
        async for line in r.aiter_lines():
            if line.startswith(":"):
                continue
            if line:
                key, _, value = line.partition(":")
                fields[key] = value[1:] if value.startswith(" ") else value
                continue
            if not fields:
                continue
            event, data = fields.get("event", "message"), json.loads(fields["data"]) if "data" in fields else None
            fields = {}
            if event == "heartbeat":
                continue
            out.append((time.perf_counter() - t0, event, data))
            if event in ("done", "error"):
                break
    return out


def first(events: list[tuple[float, str, Any]], pred: Callable[[str, Any], bool]) -> float | None:
    return next((round(t, 3) for t, e, d in events if pred(e, d)), None)


async def run_case(http: httpx.AsyncClient, headers: dict[str, str], app: Any, case: dict[str, Any],
                   mode: str) -> dict[str, Any]:
    row: dict[str, Any] = {
        "id": case["id"], "prompt": case["prompt"], "level": case["level"], "expect": case["expect"],
        "tags": case.get("tags") or [], "job_id": None, "state": "not_started", "error": None, "plan": None,
        "blocks": [], "tokens": {"in": 0, "out": 0}, "model": None,
        "times": {"first_narration": None, "first_op": None, "total": None},
    }
    r = await http.post("/v1/projects", json={"title": f"eval {case['id']}"}, headers=headers)
    r.raise_for_status()
    pid = r.json()["id"]
    t0 = time.perf_counter()
    r = await http.post(f"/v1/projects/{pid}/generate", headers=headers,
                        json={"prompt": case["prompt"], "level": case["level"], "mode": mode})
    if r.status_code != 202:
        err = r.json()
        row["error"] = {"code": err.get("code") or f"http_{r.status_code}", "message": err.get("message") or r.text}
        return row
    jid = r.json()["job_id"]
    events = await read_events(http, jid, headers, t0)

    async with app.state.engine.connect() as conn:
        job = (await conn.execute(select(jobs).where(jobs.c.id == jid))).one()
        tries = (await conn.execute(select(block_attempts).where(block_attempts.c.job_id == jid)
                                    .order_by(block_attempts.c.block_id, block_attempts.c.attempt))).all()
    checks = {d["block"]: d["checks"] for _, e, d in events if e == "sim.summary"}
    plan_blocks = (job.plan or {}).get("blocks") or []
    row.update(
        job_id=jid, state=job.state, model=job.model, tokens={"in": job.in_tokens, "out": job.out_tokens},
        verification=(job.plan or {}).get("verification"),
        assembly_attempts=(job.plan or {}).get("assembly_attempts", []),
        error=next(({"code": d["code"], "message": d["message"]} for _, e, d in events if e == "error"), None),
        plan={"rounds": job.plan.get("rounds"), "templates": [b["template"] for b in plan_blocks]} if job.plan else None,
        blocks=[
            {
                "id": b["id"], "template": b["template"], "role": b["role"],
                "how": b["outcome"].get("how"), "attempts": b["outcome"].get("attempts"),
                "errors": b["outcome"].get("errors") or [], "why": b["outcome"].get("why"),
                "checks": [{k: c.get(k) for k in ("name", "target", "measured", "pass")} for c in checks.get(b["id"], [])],
                "attempt_ms": [t.latency_ms for t in tries if t.block_id == b["id"]],
            }
            for b in plan_blocks
        ],
        times={
            "first_narration": first(events, lambda e, d: e == "narration.delta" and "block" not in d),
            "first_op": first(events, lambda e, d: e == "op"),
            "total": round(events[-1][0], 3) if events else None,
        },
    )
    return row


async def evaluate(cases: list[dict[str, Any]], gateway: Gateway, *, database_url: str, redis_url: str,
                   mode: str = "compose", concurrency: int = 4, job_timeout_s: float = 120.0,
                   progress: Callable[[str], None] = lambda line: print(line, flush=True)) -> tuple[list[dict[str, Any]], str]:
    """Every case as a job on a fresh project, `concurrency` at a time; returns the rows in case
    order and the registry version."""
    settings = Settings(database_url=database_url, redis_url=redis_url, auth_secret=secrets.token_urlsafe(32),
                        job_timeout_s=job_timeout_s)
    app = create_app(settings, Orchestrator(gateway))
    gate_ = asyncio.Semaphore(concurrency)
    finished = 0

    async with stack.sim_worker(redis_url, max_jobs=max(4, concurrency)), stack.api(app) as (url, _):
        async with httpx.AsyncClient(base_url=url, timeout=httpx.Timeout(60, read=180)) as http:
            r = await http.post("/v1/auth/anonymous")
            r.raise_for_status()
            headers = {"Authorization": f"Bearer {r.json()['token']}"}

            async def one(case: dict[str, Any]) -> dict[str, Any]:
                nonlocal finished
                async with gate_:
                    row = await run_case(http, headers, app, case, mode)
                finished += 1
                hows = " ".join(f"{b['template']}:{b['how'] or '-'}/{b['attempts'] or 0}" for b in row["blocks"])
                code = f" {row['error']['code']}" if row["error"] else ""
                progress(f"[{finished}/{len(cases)}] {case['id']}: {row['state']}{code} {hows} "
                         f"({row['times']['total'] or 0:.1f} s)")
                return row

            rows = await asyncio.gather(*(one(c) for c in cases))
        version = app.state.regs.current.version
    return list(rows), version


def git_head() -> str | None:
    try:
        return subprocess.run(["git", "rev-parse", "--short", "HEAD"], cwd=REPO, capture_output=True, text=True,
                              check=True).stdout.strip()
    except (OSError, subprocess.CalledProcessError):
        return None


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0], formatter_class=argparse.RawDescriptionHelpFormatter)
    src = ap.add_mutually_exclusive_group()
    src.add_argument("--live", action="store_true", help="allow real model calls (the provider in LLM_PROVIDER)")
    src.add_argument("--replay", type=Path, metavar="CASSETTE", help="answer from a recorded cassette")
    src.add_argument("--fake", type=Path, metavar="SCRIPT", help="answer from a FakeProvider script")
    src.add_argument("--exchange", type=Path, metavar="DIR",
                     help="answer from replies saved in DIR/replies; unanswered calls go to DIR/pending (exchange.py)")
    ap.add_argument("--label", default="chat", help="with --exchange: who answered (the model and where)")
    ap.add_argument("--record", action="store_true", help="record every model call to <out>/cassette.json")
    ap.add_argument("--env-file", type=Path, help="read KEY=VALUE lines (e.g. .env) without overriding the environment")
    ap.add_argument("--prompts", type=Path, default=HERE / "prompts.yaml")
    ap.add_argument("--only", help="comma-separated prompt ids")
    ap.add_argument("--tags", help="comma-separated tags: prompts with any of them")
    ap.add_argument("--limit", type=int)
    ap.add_argument("--mode", choices=("compose", "templates"), default="compose")
    ap.add_argument("--concurrency", type=int, default=4)
    ap.add_argument("--max-wait", type=float, default=None,
                    help="seconds a call may wait out rate limits (default 900 with --live, else 0)")
    ap.add_argument("--job-timeout", type=float, default=None,
                    help="job time limit in seconds (default 120, LLD 1; 3600 with --live, since waits count)")
    ap.add_argument("--out", type=Path, help="report directory (default evals/results/<time>-<provider>)")
    ap.add_argument("--gate", action="store_true", help="exit 1 unless the run passes the gate (metrics.gate)")
    ap.add_argument("--baseline", type=Path, help="an accepted report.json for the gate's regression rules")
    ap.add_argument("--min-commit", type=float, default=None, help="gate floor for blocks without fallback (0.80)")
    args = ap.parse_args(argv)
    if hasattr(sys.stdout, "reconfigure"):
        sys.stdout.reconfigure(encoding="utf-8", errors="replace")  # the report's ✅ and ≥ on a Windows console

    if args.env_file:
        load_env_file(args.env_file)
    cases = load_cases(args.prompts, only=args.only.split(",") if args.only else None,
                       tags=args.tags.split(",") if args.tags else None, limit=args.limit)
    providers, label = providers_from_args(args)
    max_wait = args.max_wait if args.max_wait is not None else (900.0 if args.live else 0.0)
    job_timeout = args.job_timeout if args.job_timeout is not None else (3600.0 if args.live else 120.0)
    m = models(providers, record=args.record, label=label, max_wait_s=max_wait)
    started = datetime.now(UTC)
    out = args.out or HERE / "results" / f"{started:%Y%m%dT%H%M%SZ}-{m.name}"
    print(f"{len(cases)} prompts on {m.description}, {args.concurrency} at a time; report in {out}", flush=True)

    t0 = time.perf_counter()
    with stack.databases() as (database_url, redis_url):
        rows, version = asyncio.run(evaluate(cases, m.gateway, database_url=database_url, redis_url=redis_url,
                                             mode=args.mode, concurrency=args.concurrency, job_timeout_s=job_timeout))
    waits = m.rate_limit_waits()
    # A job's time includes any rate-limit waits; then only the per-call model latency means anything.
    metrics = summarize(rows, m.calls, latency_measured=args.live and not waits["waits"])
    baseline = json.loads(args.baseline.read_text(encoding="utf-8"))["metrics"] if args.baseline else None
    problems = None
    if args.gate:
        kw = {"min_commit": args.min_commit} if args.min_commit is not None else {}
        problems = gate(metrics, baseline, **kw)
    report = {
        "meta": {"started": started.isoformat(timespec="seconds"), "provider": m.description, "mode": args.mode,
                 "registry_version": version, "git": git_head(), "prompts_file": str(args.prompts),
                 "concurrency": args.concurrency, "job_timeout_s": job_timeout, "rate_limit_waits": waits,
                 "wall_s": round(time.perf_counter() - t0, 1)},
        "metrics": metrics, "gate": problems, "rows": rows, "calls": m.calls,
    }
    out.mkdir(parents=True, exist_ok=True)
    (out / "report.json").write_text(json.dumps(report, indent=1, ensure_ascii=False) + "\n", encoding="utf-8")
    (out / "report.md").write_text(markdown(report), encoding="utf-8")
    if args.record:
        m.save_cassette(out / "cassette.json")
    print(markdown(report).split("\n## Jobs")[0], flush=True)
    print(f"report: {out / 'report.md'}")
    if args.exchange and (waiting := sorted(providers[0].pending)):
        print(f"\n{len(waiting)} model calls are waiting for a reply. Their prompts are in {args.exchange / 'pending'}; "
              f"save each reply as {args.exchange / 'replies'}/<same name>.txt, then run the same command again.")
        return 3
    return 1 if problems else 0


if __name__ == "__main__":
    sys.exit(main())
