"""The student test sessions' answers, read back from the database (LLD §16, the Phase 3 gate: "20
student test sessions"; how to run them is in README.md).

    python evals/sessions/export.py [--database-url URL] [--since 2026-03-11T09:00] [--until ...] [--out DIR]

Every answer in `asks` in the window, grouped by session (one anonymous user per browser profile, so
one per student), with the checks the tutor evals make: references (as saved, and the invalid ones
named by reading the answer against the circuit at its rev, folded from the op log), numeric grounding
against the context the model was given (`asks.context`), whether its experiment was tried (the
student's own ops after the answer equal the suggested ones), and the student's Yes/No. Writes
`answers.csv` (one row per answer, for reading every answer) and `sessions.md` (per session and in
all, with every invalid reference and ungrounded number listed for review).
"""

from __future__ import annotations

import argparse
import asyncio
import csv
import json
import statistics
import sys
from collections import defaultdict
from datetime import datetime
from pathlib import Path
from typing import Any

HERE = Path(__file__).resolve().parent
REPO = HERE.parents[1]
sys.path.insert(0, str(HERE.parent))

import circuit_core as cc  # noqa: E402
from sqlalchemy import select  # noqa: E402
from sqlalchemy.ext.asyncio import create_async_engine  # noqa: E402
from tutor_api.db.tables import asks, ops  # noqa: E402
from tutor_metrics import ground, pct  # noqa: E402

DEFAULT_DB = "postgresql+asyncpg://tutor:tutor@127.0.0.1:5432/tutor"  # compose's Postgres (README.md)


def _stamp(text: str | None) -> datetime | None:
    if not text:
        return None
    t = datetime.fromisoformat(text)
    return t if t.tzinfo else t.astimezone()  # a time without a zone is local


def tried(suggested: list[dict[str, Any]], later: list[dict[str, Any]]) -> bool:
    """The student applied the suggestion: their ops after the answer hold it as a consecutive run."""
    want = [(o["op"], o.get("body")) for o in suggested]
    have = [(o["op"], o.get("body")) for o in later]
    return bool(want) and any(have[i:i + len(want)] == want for i in range(len(have) - len(want) + 1))


async def load(database_url: str, since: datetime | None, until: datetime | None) -> tuple[list[Any], dict[Any, list[Any]]]:
    engine = create_async_engine(database_url)
    try:
        async with engine.connect() as conn:
            q = select(asks).order_by(asks.c.created_at)
            if since:
                q = q.where(asks.c.created_at >= since)
            if until:
                q = q.where(asks.c.created_at <= until)
            rows = (await conn.execute(q)).all()
            projects = {r.project_id for r in rows}
            log: dict[Any, list[Any]] = defaultdict(list)
            if projects:
                for o in (await conn.execute(select(ops).where(ops.c.project_id.in_(projects)).order_by(ops.c.seq))).all():
                    log[o.project_id].append(o)
    finally:
        await engine.dispose()
    return rows, log


def circuit_at(folds: dict[Any, cc.Session], reg: cc.Registry, ops_log: list[Any], project: Any, rev: int) -> cc.Session:
    """The project's circuit at `rev`, folded from its op log; kept per project, since answers come
    in time order and revs only grow."""
    s = folds.get(project)
    if s is None or s.rev > rev:
        s = folds[project] = cc.Session(reg)
    for o in ops_log:
        if s.rev < o.seq <= rev:
            cc.unwrap(s.apply(json.dumps(o.op)))
    return s


def ref_text(ref: dict[str, Any]) -> str:
    return f"[{ref['id']}]" if ref["kind"] == "part" else f"[{ref['kind']}:{ref['id']}]"


def answers(rows: list[Any], log: dict[Any, list[Any]], reg: cc.Registry) -> list[dict[str, Any]]:
    folds: dict[Any, cc.Session] = {}
    sessions: dict[Any, str] = {}
    out = []
    for r in rows:
        session = sessions.setdefault(r.user_id, f"S{len(sessions) + 1:02d}")
        a = json.loads(circuit_at(folds, reg, log[r.project_id], r.project_id, r.rev).read_answer(r.answer))
        invalid = dict.fromkeys(ref_text(ref) for ref in a["refs"] if not ref["valid"])
        t = a.get("try")
        later = [o.op for o in log[r.project_id] if o.rev_after > r.rev and o.author == "user" and o.created_at >= r.created_at]
        g = ground(a["body"], r.context, r.question if r.kind == "ask" else "") if r.context else None
        out.append({
            "session": session, "time": r.created_at.astimezone().isoformat(timespec="seconds"), "project": str(r.project_id),
            "kind": r.kind, "level": r.level, "mode": r.mode, "effort": r.effort or "normal", "question": r.question,
            "selection": json.dumps(r.selection) if r.selection else "", "answer": r.answer,
            "refs_valid": r.refs_valid, "refs_invalid": r.refs_invalid, "invalid_refs": " ".join(invalid),
            "quantities": g.quantities if g else "", "ungrounded": "; ".join(f"{u['text']} ({u['why']})" for u in g.ungrounded) if g else "",
            "try": "yes" if t else "", "try_predict": (t or {}).get("predict", ""),
            "try_applied": ("yes" if tried(t["ops"], later) else "no") if t else "",
            "feedback": {1: "yes", -1: "no"}.get(r.feedback, ""),
            "first_token_ms": "" if r.first_token_ms is None else r.first_token_ms, "ms": "" if r.ms is None else r.ms,
            "model": r.model or "", "in_tokens": r.in_tokens, "out_tokens": r.out_tokens,
        })
    return out


def summary(rows: list[dict[str, Any]]) -> dict[str, Any]:
    refs = sum(r["refs_valid"] + r["refs_invalid"] for r in rows)
    grounded = [r for r in rows if r["quantities"] != ""]
    rated = [r for r in rows if r["feedback"]]
    tries = [r for r in rows if r["try"]]
    ms = [r["ms"] for r in rows if r["ms"] != ""]
    return {
        "answers": len(rows), "what_changed": sum(1 for r in rows if r["kind"] == "what_changed"),
        "refs": refs, "refs_valid": sum(r["refs_valid"] for r in rows) / refs if refs else None,
        "grounded_answers": sum(1 for r in grounded if not r["ungrounded"]) / len(grounded) if grounded else None,
        "tries": len(tries), "tries_applied": sum(1 for r in tries if r["try_applied"] == "yes"),
        "rated": len(rated), "helpful": sum(1 for r in rated if r["feedback"] == "yes") / len(rated) if rated else None,
        "socratic": sum(1 for r in rows if r["mode"] == "socratic"),
        "answer_s_median": statistics.median(ms) / 1e3 if ms else None,
    }


def markdown(rows: list[dict[str, Any]], window: str) -> str:
    by: dict[str, list[dict[str, Any]]] = defaultdict(list)
    for r in rows:
        by[r["session"]].append(r)
    total = summary(rows)
    out = [f"# Student test sessions: answers ({window})", "",
           f"{len(by)} sessions, {total['answers']} answers. Gate criteria and how to read this: evals/sessions/README.md.", "",
           "| Session | Answers | What changed? | Socratic | Refs valid | Grounded answers | Experiments tried | Helpful (rated) | Answer time (median) |",
           "| --- | --- | --- | --- | --- | --- | --- | --- | --- |"]
    for name, rs in [*by.items(), ("**All**", rows)]:
        s = summary(rs)
        median = "–" if s["answer_s_median"] is None else f"{s['answer_s_median']:.1f} s"
        out.append(f"| {name} | {s['answers']} | {s['what_changed']} | {s['socratic']} | {pct(s['refs_valid'])} ({s['refs']}) | "
                   f"{pct(s['grounded_answers'])} | {s['tries_applied']}/{s['tries']} | {pct(s['helpful'])} ({s['rated']}) | {median} |")
    for title, pick in (("Invalid references", lambda r: r["refs_invalid"]), ("Ungrounded numbers", lambda r: r["ungrounded"]),
                        ("Answers marked not helpful", lambda r: r["feedback"] == "no")):
        hits = [r for r in rows if pick(r)]
        if hits:
            out += ["", f"## {title} ({len(hits)})", ""]
            for r in hits:
                extra = {"Invalid references": f" — {r['invalid_refs']}",
                         "Ungrounded numbers": f" — {r['ungrounded']}"}.get(title, "")
                out.append(f"- {r['session']} {r['time']} ({r['kind']}): “{r['question']}”{extra}")
    return "\n".join(out) + "\n"


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("--database-url", default=DEFAULT_DB)
    ap.add_argument("--since", help="ISO time, e.g. 2026-03-11T09:00 (local time without a zone)")
    ap.add_argument("--until", help="ISO time")
    ap.add_argument("--out", type=Path, default=HERE / "results")
    args = ap.parse_args(argv)
    if hasattr(sys.stdout, "reconfigure"):
        sys.stdout.reconfigure(encoding="utf-8", errors="replace")
    rows, log = asyncio.run(load(args.database_url, _stamp(args.since), _stamp(args.until)))
    out = answers(rows, log, cc.load_registry_dir(REPO / "registry"))
    args.out.mkdir(parents=True, exist_ok=True)
    with open(args.out / "answers.csv", "w", encoding="utf-8-sig", newline="") as f:
        w = csv.DictWriter(f, list(out[0]) if out else ["session"])
        w.writeheader()
        w.writerows(out)
    window = f"{args.since or 'start'} to {args.until or 'now'}"
    (args.out / "sessions.md").write_text(markdown(out, window), encoding="utf-8")
    print(markdown(out, window).split("\n## ")[0])
    print(f"wrote {args.out / 'answers.csv'} and {args.out / 'sessions.md'}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
