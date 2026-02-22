"""Tutor evals (LLD §9, §15; the Phase 3 gate, LLD §16): every case in evals/tutor/cases.yaml is asked
through the real API, on Postgres and Redis in testcontainers (tools/e2e/stack.py). The case's circuit
is built from its frozen ops (evals/tutor/fixtures.json), then `POST /ask` or `POST /what-changed` is
sent with the frozen simulation values the editor would send, and the answer is read from the SSE
stream. Each answer is then checked: references (circuit-core's reading), numeric grounding, and its
`try` block applied and simulated again on the native ngspice. The metrics, the gate and the report are
in tutor_metrics.py; the LLM rubric in tutor_judge.py.

    python evals/run_tutor_evals.py --env-file .env --live --record --rubric --effort low   # the provider in .env, recorded
    python evals/run_tutor_evals.py --replay evals/tutor_golden/cassette.json --gate --baseline evals/tutor_golden/report.json
    python evals/run_tutor_evals.py --fake SCRIPT.json      # scripted replies (LLM_PROVIDER=fake's format)

`--tier small|large` answers both kinds on that tier; `--effort low|high` is what the learner picks as
Quick or Deep thinking (none: Normal, the provider's default); a replay with --baseline takes both, and
--rubric, from that report unless given. `--rubric` grades the cases tagged
`rubric` with the judge (`--judge`, a provider name as in apps/api/tutor_api/llm/config.py; with
--live, by default deepseek; otherwise the same replies as the tutor's) and writes review.csv for a
human reviewer. Each run writes report.json, report.md and, with --record, cassette.json to
evals/results/<time>-tutor-<provider>/. Exit status 1 when --gate fails, 3 while an exchange run waits.
"""

from __future__ import annotations

import argparse
import asyncio
import csv
import json
import os
import secrets
import sys
import time
from collections.abc import Callable
from datetime import UTC, datetime
from pathlib import Path
from typing import Any

import httpx
import yaml

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE / "tutor"))

from run_evals import git_head, load_env_file, models, providers_from_args  # noqa: E402  (puts tools/e2e on the path)

import circuit_core as cc  # noqa: E402
import fixtures as fx  # noqa: E402
import stack  # noqa: E402
from sqlalchemy import select  # noqa: E402
from tutor_api.config import Settings  # noqa: E402
from tutor_api.db.tables import asks  # noqa: E402
from tutor_api.llm.config import provider_from_env  # noqa: E402
from tutor_api.llm.gateway import Gateway  # noqa: E402
from tutor_api.main import create_app  # noqa: E402
from tutor_api.orchestrator import Orchestrator  # noqa: E402
from tutor_api.tutor import Tutor  # noqa: E402
from tutor_judge import judge_request, read_verdict  # noqa: E402
from tutor_metrics import REVIEW_COLUMNS, gate, markdown, read_row, review_rows, summarize  # noqa: E402

# ---------------------------------------------------------------- cases


def load_cases(path: Path, *, only: list[str] | None = None, tags: list[str] | None = None,
               kind: str | None = None, limit: int | None = None) -> list[dict[str, Any]]:
    cases = yaml.safe_load(path.read_text(encoding="utf-8"))
    for c in cases:
        c.setdefault("kind", "ask")
        c.setdefault("mode", "explain")
    if only:
        missing = set(only) - {c["id"] for c in cases}
        if missing:
            raise SystemExit(f"no case {', '.join(sorted(missing))} in {path}")
        cases = [c for c in cases if c["id"] in only]
    if tags:
        cases = [c for c in cases if set(tags) & set(c.get("tags") or ())]
    if kind:
        cases = [c for c in cases if c["kind"] == kind]
    return cases[:limit] if limit else cases


def request(case: dict[str, Any], fixtures: dict[str, Any], effort: str | None) -> tuple[str, dict[str, Any]]:
    """The path and body the editor would send for `case`."""
    circuit = fixtures["circuits"][case["circuit"]]
    common = {"level": case["level"], "mode": case["mode"], **({"effort": effort} if effort else {})}
    if case["kind"] == "what_changed":
        edit = fixtures["edits"][case["id"]]
        return "what-changed", {"from_rev": circuit["rev"], "rev": edit["rev"], "before": circuit["sim"],
                                "after": edit["sim"], **common}
    body = {"question": case["question"], "rev": circuit["rev"], "sim": circuit["sim"], **common}
    if case.get("selection"):
        body["selection"] = case["selection"]
    return "ask", body


def batches(case: dict[str, Any], fixtures: dict[str, Any]) -> list[dict[str, Any]]:
    """The op batches of the case's circuit, and of its edit for "What changed?"."""
    out = list(fixtures["circuits"][case["circuit"]]["batches"])
    if case["kind"] == "what_changed":
        out.append({"author": "user", "ops": fixtures["edits"][case["id"]]["ops"]})
    return out


# ---------------------------------------------------------------- asking


async def sse(r: httpx.Response, t0: float) -> list[tuple[float, str, Any]]:
    out: list[tuple[float, str, Any]] = []
    fields: dict[str, str] = {}
    async for line in r.aiter_lines():
        if line.startswith(":"):
            continue
        if line:
            key, _, value = line.partition(":")
            fields[key] = value[1:] if value.startswith(" ") else value
            continue
        if fields:
            out.append((time.perf_counter() - t0, fields.get("event", "message"),
                        json.loads(fields["data"]) if "data" in fields else None))
            fields = {}
    return out


async def run_case(http: httpx.AsyncClient, headers: dict[str, str], app: Any, case: dict[str, Any],
                   fixtures: dict[str, Any], effort: str | None) -> dict[str, Any]:
    row: dict[str, Any] = {
        "id": case["id"], "kind": case["kind"], "circuit": case["circuit"], "category": case["category"],
        "level": case["level"], "mode": case["mode"], "tags": case.get("tags") or [], "expect": case.get("expect") or {},
        "question": case.get("question"), "selection": case.get("selection"), "error": None, "text": None,
        "answer": None, "usage": None, "model": None, "times": {"first_token": None, "total": None},
    }
    r = await http.post("/v1/projects", json={"title": f"tutor eval {case['id']}"}, headers=headers)
    r.raise_for_status()
    pid = r.json()["id"]
    r = await http.post(f"/v1/projects/{pid}/ops", json={"base_rev": 0, "ops": fx.envelopes(batches(case, fixtures))},
                        headers=headers)
    r.raise_for_status()
    path, body = request(case, fixtures, effort)
    t0 = time.perf_counter()
    async with http.stream("POST", f"/v1/projects/{pid}/{path}", json=body, headers=headers) as resp:
        if resp.status_code != 200:
            err = json.loads(await resp.aread())
            row["error"] = {"code": err.get("code") or f"http_{resp.status_code}", "message": err.get("message")}
            return row
        events = await sse(resp, t0)
    deltas = [t for t, e, _ in events if e == "answer.delta"]
    row["times"] = {"first_token": round(deltas[0], 3) if deltas else None, "total": round(events[-1][0], 3) if events else None}
    row["text"] = "".join(d["text"] for _, e, d in events if e == "answer.delta")
    end = events[-1] if events else (0, "error", {"code": "no_events", "message": "the stream ended without an event"})
    if end[1] != "answer.done":
        row["error"] = {"code": end[2].get("code"), "message": end[2].get("message")}
        return row
    done = end[2]
    row.update(answer=done["answer"], usage=done["usage"])
    async with app.state.engine.connect() as conn:
        row["model"] = (await conn.execute(select(asks.c.model).where(asks.c.id == done["ask_id"]))).scalar_one()
    return row


# ---------------------------------------------------------------- checking


def context(reg: cc.Registry, case: dict[str, Any], fixtures: dict[str, Any], body: dict[str, Any]) -> dict[str, Any]:
    """The context the server built for this request: the same circuit-core call on the same circuit."""
    before = fx.session_at(reg, fixtures["circuits"][case["circuit"]]["batches"])
    if case["kind"] == "what_changed":
        after = fx.session_at(reg, batches(case, fixtures))
        return cc.unwrap(after.tutor_changes(before, json.dumps(body)))
    return cc.unwrap(before.tutor_context(json.dumps(body)))


def measure(session: cc.Session) -> dict[str, Any]:
    """The circuit's spec checks on the native ngspice, simulated as the editor simulates it."""
    from sim_runner import ngspice_batch

    n = cc.unwrap(session.compile(json.dumps({"shunt_floating": True, "interactive": True})))
    r = ngspice_batch.simulate(n["text"], n["includes"], hash=n["hash"], vectors=False)
    checks = cc.unwrap(cc.evaluate_checks(json.dumps(n["checks"]), json.dumps(r.meas))) if r.status == "ok" else []
    return {"status": r.status, "checks": checks}


def tried(reg: cc.Registry, case: dict[str, Any], fixtures: dict[str, Any], ops: list[dict[str, Any]]) -> dict[str, Any]:
    """The experiment applied to the circuit the answer was about (one `user` batch, as Try it does)
    and both circuits simulated: the spec checks before and after. A source block's checks measure
    the stimulus, not what the circuit did with it, so they are left out."""
    session = fx.session_at(reg, batches(case, fixtures))
    blocks = json.loads(session.snapshot())["blocks"]
    sources = {b for b, block in blocks.items() if block.get("role") == "source"}

    def response(checks: list[dict[str, Any]]) -> list[dict[str, Any]]:
        return [c for c in checks if c["block"] not in sources]

    before = response(measure(session)["checks"])
    trial = session.fork()
    try:
        cc.unwrap(trial.apply_ops(json.dumps(ops), "user"))
    except cc.OpRejected as e:
        return {"status": "refused", "error": str(e), "before": before, "after": []}
    try:
        after = measure(trial)
    except cc.OpRejected as e:  # it does not compile
        return {"status": "compile_error", "error": str(e), "before": before, "after": []}
    return {"status": after["status"], "before": before, "after": response(after["checks"])}


def check(rows: list[dict[str, Any]], cases: dict[str, dict[str, Any]], fixtures: dict[str, Any], reg: cc.Registry,
          effort: str | None) -> None:
    """Context, re-simulated experiment and the automatic checks, onto each answered row."""
    for row in rows:
        if row["answer"] is None:
            continue
        case = cases[row["id"]]
        ctx = context(reg, case, fixtures, request(case, fixtures, effort)[1])
        row["context"] = {k: ctx[k] for k in ("text", "parts", "nets", "blocks", "tokens")}
        t = row["answer"].get("try")
        if t is not None and not t.get("problems"):
            row["tried"] = tried(reg, case, fixtures, t["ops"])
        row["checks"] = read_row(row)


async def grade(rows: list[dict[str, Any]], judge: Gateway, concurrency: int,
                progress: Callable[[str], None]) -> None:
    """The judge's rubric on every answered row tagged `rubric`."""
    sample = [r for r in rows if "rubric" in r["tags"] and r.get("checks")]
    sem = asyncio.Semaphore(concurrency)
    finished = 0

    async def one(row: dict[str, Any]) -> None:
        nonlocal finished
        async with sem:
            try:
                resp = await judge.complete(judge_request(row))
                row["rubric"] = read_verdict(resp.text)
            except Exception as e:  # noqa: BLE001  (a judge that fails leaves the row ungraded, and says why)
                row["rubric"] = {"error": type(e).__name__, "raw": str(e)[:300]}
        finished += 1
        progress(f"  graded [{finished}/{len(sample)}] {row['id']}: {row['rubric'].get('verdict') or row['rubric'].get('error')}")

    await asyncio.gather(*(one(r) for r in sample))


# ---------------------------------------------------------------- the run


async def evaluate(cases: list[dict[str, Any]], gateway: Gateway, *, database_url: str, redis_url: str,
                   fixtures: dict[str, Any], tiers: dict[str, str], effort: str | None = None, concurrency: int = 4,
                   judge: Gateway | None = None, ask_timeout_s: float = 180.0,
                   progress: Callable[[str], None] = lambda line: print(line, flush=True)) -> tuple[list[dict[str, Any]], str]:
    """Every case asked on a fresh project, `concurrency` at a time, then checked and (with `judge`)
    graded; returns the rows in case order and the registry version."""
    settings = Settings(database_url=database_url, redis_url=redis_url, auth_secret=secrets.token_urlsafe(32),
                        ask_timeout_s=ask_timeout_s)
    app = create_app(settings, Orchestrator(gateway), Tutor(gateway, tiers))
    sem = asyncio.Semaphore(concurrency)
    finished = 0
    async with stack.api(app) as (url, _):
        if app.state.regs.current.version != fixtures["registry_version"]:
            raise SystemExit(f"fixtures.json is for registry {fixtures['registry_version']}, the API runs "
                             f"{app.state.regs.current.version}: run evals/tutor/fixtures.py")
        async with httpx.AsyncClient(base_url=url, timeout=httpx.Timeout(60, read=ask_timeout_s + 30)) as http:
            r = await http.post("/v1/auth/anonymous")
            r.raise_for_status()
            headers = {"Authorization": f"Bearer {r.json()['token']}"}

            async def one(case: dict[str, Any]) -> dict[str, Any]:
                nonlocal finished
                async with sem:
                    row = await run_case(http, headers, app, case, fixtures, effort)
                finished += 1
                a = row["answer"]
                what = (f"refs {a['refs_valid']}/{a['refs_valid'] + a['refs_invalid']}{' try' if a.get('try') else ''}"
                        if a else (row["error"] or {}).get("code"))
                progress(f"[{finished}/{len(cases)}] {case['id']}: {what} ({row['times']['total'] or 0:.1f} s)")
                return row

            rows = list(await asyncio.gather(*(one(c) for c in cases)))
        reg = app.state.regs.current
        version = app.state.regs.current.version
    check(rows, {c["id"]: c for c in cases}, fixtures, reg, effort)
    if judge is not None:
        await grade(rows, judge, concurrency, progress)
    return rows, version


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0], formatter_class=argparse.RawDescriptionHelpFormatter)
    src = ap.add_mutually_exclusive_group()
    src.add_argument("--live", action="store_true", help="allow real model calls (the provider in LLM_PROVIDER)")
    src.add_argument("--replay", type=Path, metavar="CASSETTE", help="answer from a recorded cassette")
    src.add_argument("--fake", type=Path, metavar="SCRIPT", help="answer from a FakeProvider script")
    src.add_argument("--exchange", type=Path, metavar="DIR", help="answer from replies saved in DIR/replies (exchange.py)")
    ap.add_argument("--label", default="chat", help="with --exchange: who answered (the model and where)")
    ap.add_argument("--record", action="store_true", help="record every model call to <out>/cassette.json")
    ap.add_argument("--env-file", type=Path, help="read KEY=VALUE lines (e.g. .env) without overriding the environment")
    ap.add_argument("--cases", type=Path, default=fx.CASES)
    ap.add_argument("--only", help="comma-separated case ids")
    ap.add_argument("--tags", help="comma-separated tags: cases with any of them")
    ap.add_argument("--kind", choices=("ask", "what_changed"))
    ap.add_argument("--limit", type=int)
    ap.add_argument("--tier", choices=("small", "large"), help="the tier both kinds answer on (default: LLM_TIER_* or small)")
    ap.add_argument("--effort", choices=("low", "high"), help="the reasoning effort (default: the provider's)")
    ap.add_argument("--rubric", action="store_true", help="grade the cases tagged rubric with the judge")
    ap.add_argument("--judge", default=None, help="the judge's provider with --live or --judge-live (default deepseek)")
    ap.add_argument("--judge-live", action="store_true",
                    help="grade with the real judge although the tutor's replies come from --replay (regrading a recorded run)")
    ap.add_argument("--concurrency", type=int, default=4)
    ap.add_argument("--max-wait", type=float, default=None,
                    help="seconds a call may wait out rate limits (default 900 with --live, else 0)")
    ap.add_argument("--out", type=Path, help="report directory (default evals/results/<time>-tutor-<provider>)")
    ap.add_argument("--gate", action="store_true", help="exit 1 unless the run passes the gate (tutor_metrics.gate)")
    ap.add_argument("--baseline", type=Path, help="an accepted report.json for the gate's regression rules")
    args = ap.parse_args(argv)
    if hasattr(sys.stdout, "reconfigure"):
        sys.stdout.reconfigure(encoding="utf-8", errors="replace")

    if args.env_file:
        load_env_file(args.env_file)
    cases = load_cases(args.cases, only=args.only.split(",") if args.only else None,
                       tags=args.tags.split(",") if args.tags else None, kind=args.kind, limit=args.limit)
    fixtures = json.loads(fx.FIXTURES.read_text(encoding="utf-8"))
    base = json.loads(args.baseline.read_text(encoding="utf-8")) if args.baseline else None
    if args.replay and base:
        # A replay asks what the recorded run asked: its tiers, effort and rubric, unless given.
        args.tier = args.tier or (base["meta"]["tiers"]["ask"] if len(set(base["meta"]["tiers"].values())) == 1 else None)
        args.effort = args.effort or base["meta"].get("effort")
        args.rubric = args.rubric or base["meta"].get("judge") is not None
    providers, label = providers_from_args(args)
    max_wait = args.max_wait if args.max_wait is not None else (900.0 if args.live or args.judge_live else 0.0)
    m = models(providers, record=args.record, label=label, max_wait_s=max_wait)
    judge = None
    if args.rubric:
        live_judge = args.live or args.judge_live
        judges = [provider_from_env(args.judge or "deepseek", os.environ, primary=False)] if live_judge else providers
        jm = models(judges, record=args.record, max_wait_s=max_wait)
        if args.record:
            jm.recorders[0].interactions = m.recorders[0].interactions  # one cassette for the run
        judge = jm.gateway
    from tutor_api.llm.config import tutor_tiers_from_env

    tiers = {k: args.tier for k in ("ask", "what_changed")} if args.tier else tutor_tiers_from_env()
    started = datetime.now(UTC)
    out = args.out or HERE / "results" / f"{started:%Y%m%dT%H%M%SZ}-tutor-{m.name}"
    print(f"{len(cases)} cases on {m.description} ({tiers}, effort {args.effort or 'default'}), "
          f"{args.concurrency} at a time; report in {out}", flush=True)

    t0 = time.perf_counter()
    with stack.databases() as (database_url, redis_url):
        rows, version = asyncio.run(evaluate(
            cases, m.gateway, database_url=database_url, redis_url=redis_url, fixtures=fixtures, tiers=tiers,
            effort=args.effort, concurrency=args.concurrency, judge=judge,
            ask_timeout_s=180.0 + max_wait))  # rate-limit waits count toward an answer's time
    waits = m.rate_limit_waits()
    calls = m.calls + (jm.calls if judge is not None else [])
    metrics = summarize(rows, calls, latency_measured=args.live and not waits["waits"])
    problems = gate(metrics, base["metrics"] if base else None) if args.gate else None
    report = {
        "meta": {"started": started.isoformat(timespec="seconds"), "provider": m.description,
                 "judge": jm.description if judge is not None else None, "tiers": tiers, "effort": args.effort,
                 "registry_version": version, "git": git_head(), "cases_file": str(args.cases),
                 "concurrency": args.concurrency, "rate_limit_waits": waits, "wall_s": round(time.perf_counter() - t0, 1)},
        "metrics": metrics, "gate": problems, "rows": rows, "calls": calls,
    }
    out.mkdir(parents=True, exist_ok=True)
    (out / "report.json").write_text(json.dumps(report, indent=1, ensure_ascii=False) + "\n", encoding="utf-8")
    (out / "report.md").write_text(markdown(report), encoding="utf-8")
    if judge is not None:
        with open(out / "review.csv", "w", encoding="utf-8-sig", newline="") as f:
            w = csv.DictWriter(f, REVIEW_COLUMNS)
            w.writeheader()
            w.writerows(review_rows(rows))
    if args.record:
        m.save_cassette(out / "cassette.json")
    print(markdown(report).split("\n## By kind")[0], flush=True)
    print(f"report: {out / 'report.md'}")
    if args.exchange and (waiting := sorted(providers[0].pending)):
        print(f"\n{len(waiting)} model calls are waiting for a reply. Their prompts are in {args.exchange / 'pending'}; "
              f"save each reply as {args.exchange / 'replies'}/<same name>.txt, then run the same command again.")
        return 3
    return 1 if problems else 0


if __name__ == "__main__":
    sys.exit(main())
