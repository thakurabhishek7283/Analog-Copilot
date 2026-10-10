"""The generation eval metrics (LLD §15), the gate and the report, from the rows run_evals.py
collects: one row per prompt, holding its job's outcome, plan, blocks, tokens and timings. Pure
functions, no I/O.

A block's outcome is `jobs.plan.blocks[].outcome.how` (LLD §6, as built): `draft` (the model's own
block passed), `use_template` (the model kept the template; it counts as the model's choice),
`fallback` (three failed attempts, or the composer's provider gave up), or `template_mode`.

The gate counts `use_template` as success: keeping a verified template is the right answer when it does
what the student asked. How well the model drafts is reported next to it, not gated: the share of blocks
it drafted, and of the blocks where it tried a draft (one passed, or an attempt failed), the share that
committed as its own draft.
"""

from __future__ import annotations

import math
from collections import Counter
from typing import Any

# LLD §16, Phase 2 gate: blocks committed without template fallback.
PHASE2_GATE = 0.80
# LLD §15, CI gate: a change fails if the commit rate drops by more than 2 points or tokens per
# circuit rise by more than 15%.
MAX_COMMIT_DROP = 0.02
MAX_TOKEN_RISE = 0.15

# LLD §15 targets (and §1 budgets) the report holds the metrics against: (key, label, target, kind).
TARGETS = [
    ("commit_without_fallback", "Blocks committed without template fallback", 0.90, "min"),
    ("first_attempt_pass", "First-attempt pass rate", 0.70, "min"),
    ("mean_attempts", "Mean attempts per block", 1.4, "max"),
    ("spec_error", "Spec error, mean relative error of the checks", 0.05, "max"),
    ("tokens_in_mean", "Input tokens per circuit (mean)", 40_000, "max"),
    ("tokens_out_mean", "Output tokens per circuit (mean)", 6_000, "max"),
    ("latency_total_p95", "Full circuit latency, p95 (s)", 25.0, "max"),
    ("latency_first_narration_p95", "First narration token, p95 (s)", 1.5, "max"),
    ("latency_first_block_p95", "First committed block, p95 (s)", 6.0, "max"),
]

MODEL_CHOICES = ("draft", "use_template")


def plan_matches(expect: list[Any], templates: list[str]) -> bool:
    """Every expected item (a template id, or a list of acceptable ids) has a block of its own."""
    wanted = [[e] if isinstance(e, str) else list(e) for e in expect]

    def fit(i: int, left: list[str]) -> bool:
        if i == len(wanted):
            return True
        return any(fit(i + 1, left[:j] + left[j + 1:]) for j, t in enumerate(left) if t in wanted[i])

    return fit(0, list(templates))


def percentile(values: list[float], p: float) -> float | None:
    """Nearest-rank percentile; None for no values."""
    if not values:
        return None
    s = sorted(values)
    return s[max(0, math.ceil(p / 100 * len(s)) - 1)]


def mean(values: list[float]) -> float | None:
    return sum(values) / len(values) if values else None


def ratio(n: int, d: int) -> float | None:
    return n / d if d else None


def in_scope(row: dict[str, Any]) -> bool:
    return row["expect"] != "unsupported"


def unanswered(rows: list[dict[str, Any]], calls: list[dict[str, Any]] | None, error: str) -> dict[str, Any]:
    """Model calls that got no reply, by the exception they raised: `CassetteMiss` (a replay asked
    for a call its cassette does not hold: a prompt, template or code change since it was
    recorded) or `AwaitingReply` (an exchange run's call nobody has answered yet). Either fails its
    job, except in narration, which the orchestrator only logs; `calls` sees both."""
    return {
        "prompts": [r["id"] for r in rows if r.get("error") and error in (r["error"].get("message") or "")],
        "calls": sum(1 for c in calls or () if c.get("error") == error),
    }


def block_stats(blocks: list[dict[str, Any]]) -> dict[str, Any]:
    n = len(blocks)
    how = Counter(b["how"] for b in blocks)
    tried = sum(1 for b in blocks if b["how"] == "draft" or b.get("errors"))
    return {
        "blocks": n,
        "commit_without_fallback": ratio(sum(how[h] for h in MODEL_CHOICES), n),
        "draft_share": ratio(how["draft"], n),
        "drafts_tried": tried,
        "draft_pass": ratio(how["draft"], tried),
        "first_attempt_pass": ratio(sum(1 for b in blocks if b["how"] in MODEL_CHOICES and b["attempts"] == 1), n),
        "mean_attempts": mean([b["attempts"] for b in blocks]),
    }


def summarize(rows: list[dict[str, Any]], calls: list[dict[str, Any]] | None = None, *,
              latency_measured: bool = True) -> dict[str, Any]:
    """The metrics of one eval run. `calls`: every model call the run made (kind, tokens, ms, error,
    whether the tokens were estimated). `latency_measured`: the replies came from a model as it ran
    (a live run), not from a recording, a script or a person."""
    scoped = [r for r in rows if in_scope(r)]
    oos = [r for r in rows if not in_scope(r)]
    done = [r for r in scoped if r["state"] == "done"]
    # Blocks with an outcome, in compose mode: what the gate counts. A failed job's committed
    # blocks count too; its blocks that never got an outcome do not.
    blocks = [b | {"level": r["level"]} for r in scoped for b in r["blocks"] if b.get("how") and b["how"] != "template_mode"]
    checks = [c for r in scoped for b in r["blocks"] for c in b.get("checks") or []]
    assemblies = [r["verification"] for r in scoped if r.get("verification")]
    errors = [
        (r["id"], (r["error"] or {}).get("code") or r["state"]) for r in scoped if r["state"] != "done"
    ]

    rel = [abs(c["measured"] - c["target"]) / abs(c["target"])
           for c in checks if c.get("measured") is not None and c.get("target")]
    m: dict[str, Any] = {
        "prompts": len(rows),
        "in_scope": len(scoped),
        "done": len(done),
        "done_rate": ratio(len(done), len(scoped)),
        "failed": dict(Counter(code for _, code in errors)),
        "out_of_scope": len(oos),
        "out_of_scope_refused": sum(1 for r in oos if (r["error"] or {}).get("code") == "unsupported_request"),
        "plan_match": ratio(sum(1 for r in scoped if r["plan"] and plan_matches(r["expect"], r["plan"]["templates"])),
                            len(scoped)),
        "plan_rounds_mean": mean([r["plan"]["rounds"] for r in scoped if r["plan"]]),
        **block_stats(blocks),
        "fallback_why": dict(Counter(b.get("why") or "attempts" for b in blocks if b["how"] == "fallback")),
        "attempt_errors": dict(Counter(code for b in blocks for codes in b.get("errors") or [] for code in codes)),
        "checks": len(checks),
        "checks_passed": ratio(sum(1 for c in checks if c.get("pass")), len(checks)),
        "assembly_outcomes": dict(Counter(a["status"] for a in assemblies)),
        "assembly_pass_rate": ratio(sum(a["status"] == "passed" for a in assemblies), len(assemblies)),
        "assembly_repairs": sum(a["attempt"] for a in assemblies),
        "spec_error": mean(rel),
        "tokens_in_mean": mean([r["tokens"]["in"] for r in done]),
        "tokens_out_mean": mean([r["tokens"]["out"] for r in done]),
        "tokens_in_p95": percentile([r["tokens"]["in"] for r in done], 95),
        "tokens_out_p95": percentile([r["tokens"]["out"] for r in done], 95),
        "latency_total_p95": percentile([r["times"]["total"] for r in done], 95),
        "latency_first_narration_p95": percentile(
            [r["times"]["first_narration"] for r in done if r["times"]["first_narration"] is not None], 95),
        "latency_first_block_p95": percentile(
            [r["times"]["first_op"] for r in done if r["times"]["first_op"] is not None], 95),
        "by_role": {role: block_stats([b for b in blocks if b["role"] == role])
                    for role in sorted({b["role"] for b in blocks})},
        "by_level": {lv: block_stats([b for b in blocks if b["level"] == lv])
                     for lv in ("beginner", "intermediate", "advanced") if any(b["level"] == lv for b in blocks)},
        "cassette_misses": unanswered(rows, calls, "CassetteMiss"),
        "awaiting": unanswered(rows, calls, "AwaitingReply"),
        "tokens_estimated": any(c.get("estimated") for c in calls) if calls else False,
        "quota_exhausted": sum(1 for c in calls or () if c.get("error") == "quota_exhausted"),
        "latency_measured": latency_measured,
    }
    if calls is not None:
        m["calls"] = call_stats(calls)
    return m


def call_stats(calls: list[dict[str, Any]]) -> dict[str, Any]:
    """Per call kind (plan, compose, narrate): calls, errors, tokens (the provider's own counts,
    cached included) and latency."""
    out = {}
    for kind in sorted({c["kind"] for c in calls}):
        cs = [c for c in calls if c["kind"] == kind]
        ok = [c for c in cs if not c.get("error")]
        out[kind] = {
            "calls": len(cs),
            "errors": dict(Counter(c["error"] for c in cs if c.get("error"))),
            "in_tokens": sum(c["in_tokens"] for c in ok),
            "out_tokens": sum(c["out_tokens"] for c in ok),
            "cached_tokens": sum(c["cached_tokens"] for c in ok),
            "in_tokens_mean": mean([c["in_tokens"] for c in ok]),
            "out_tokens_mean": mean([c["out_tokens"] for c in ok]),
            "ms_p50": percentile([c["ms"] for c in ok], 50),
            "ms_p95": percentile([c["ms"] for c in ok], 95),
            "models": sorted({c["model"] for c in ok if c.get("model")}),
        }
    return out


def gate(m: dict[str, Any], baseline: dict[str, Any] | None = None, *, min_commit: float = PHASE2_GATE) -> list[str]:
    """Why the run fails the gate; empty when it passes. `baseline`: the accepted run's metrics
    (evals/golden/report.json), for the regression rules."""
    problems = []
    wait = m.get("awaiting") or {"prompts": [], "calls": 0}
    if wait["prompts"] or wait["calls"]:
        problems.append(f"the run is not complete: {wait['calls'] or len(wait['prompts'])} model calls are waiting for "
                        "a reply (exchange)")
    if m.get("quota_exhausted"):
        problems.append(f"the provider's daily quota ran out ({m['quota_exhausted']} calls refused): the run measured "
                        "the quota, not the model")
    miss = m["cassette_misses"]
    if miss["prompts"] or miss["calls"]:
        which = f" ({', '.join(miss['prompts'][:5])})" if miss["prompts"] else ""
        problems.append(f"{miss['calls'] or len(miss['prompts'])} model calls are not in the cassette{which}: the "
                        "prompts or the pipeline changed since it was recorded; run the live evals again")
    c = m["commit_without_fallback"]
    failed_assemblies = sum(m.get("assembly_outcomes", {}).get(s, 0) for s in ("failed", "simulation_error"))
    if failed_assemblies:
        problems.append(f"{failed_assemblies} assembled circuits failed verification")
    if c is None:
        problems.append("no block got an outcome")
    elif c < min_commit:
        problems.append(f"blocks committed without template fallback: {c:.1%}, below {min_commit:.0%}")
    if baseline is not None:
        base = baseline["commit_without_fallback"]
        if c is not None and base is not None and c < base - MAX_COMMIT_DROP:
            problems.append(f"commit rate dropped {(base - c) * 100:.1f} points from {base:.1%} "
                            f"(at most {MAX_COMMIT_DROP * 100:.0f})")
        # Only measured counts compare: an estimate (characters / 4) says nothing about a provider's tokens.
        measured = not m.get("tokens_estimated") and not baseline.get("tokens_estimated")
        for key, what in (("tokens_in_mean", "input"), ("tokens_out_mean", "output")) if measured else ():
            now, then = m[key], baseline[key]
            if now is not None and then and now > then * (1 + MAX_TOKEN_RISE):
                problems.append(f"{what} tokens per circuit rose {now / then - 1:.0%}, from {then:,.0f} to {now:,.0f} "
                                f"(at most {MAX_TOKEN_RISE:.0%})")
    return problems


# ---------------------------------------------------------------- report

def fmt(v: Any, key: str = "") -> str:
    if v is None:
        return "n/a"
    if isinstance(v, float) and key in {"commit_without_fallback", "first_attempt_pass", "spec_error", "draft_share",
                                         "draft_pass", "done_rate", "plan_match", "checks_passed"}:
        return f"{v:.1%}"
    if isinstance(v, float) and "tokens" in key:
        return f"{v:,.0f}"
    if isinstance(v, float):
        return f"{v:,.2f}"
    if isinstance(v, int):
        return f"{v:,}"
    return str(v)


def meets(v: Any, target: float, kind: str) -> str:
    if v is None:
        return "–"
    return "✅" if (v >= target if kind == "min" else v <= target) else "❌"


def markdown(report: dict[str, Any]) -> str:
    m, meta, rows = report["metrics"], report["meta"], report["rows"]
    out = [
        f"# Generation evals: {meta['started']}",
        "",
        f"Provider: {meta['provider']}. Mode: {meta['mode']}. Prompts: {m['prompts']} "
        f"({m['in_scope']} in scope, {m['out_of_scope']} out of scope). Registry {meta['registry_version']}. "
        f"Wall time {meta['wall_s']:.0f} s."
        + (f" Rate limits: {meta['rate_limit_waits']['waits']} waits, {meta['rate_limit_waits']['seconds']:.0f} s in all."
           if (meta.get("rate_limit_waits") or {}).get("waits") else ""),
        "",
    ]
    wait = m.get("awaiting") or {"prompts": [], "calls": 0}
    if wait["prompts"] or wait["calls"]:
        out += [f"**Incomplete: {wait['calls'] or len(wait['prompts'])} model calls are waiting for a reply.** The metrics "
                "below cover only what has been answered so far.", ""]
    if report.get("gate") is not None:
        problems = report["gate"]
        out += ["**Gate: passed.**" if not problems else "**Gate: failed.**", ""]
        out += [f"- {p}" for p in problems] + ([""] if problems else [])
    out += [
        f"Phase 2 gate (LLD 16): blocks committed without template fallback ≥ {PHASE2_GATE:.0%}: "
        f"{fmt(m['commit_without_fallback'], 'commit_without_fallback')} "
        f"{meets(m['commit_without_fallback'], PHASE2_GATE, 'min')}",
        "",
        f"Drafting (reported, not gated): the model drafted {fmt(m['draft_share'], 'draft_share')} of blocks; "
        f"{fmt(m['draft_pass'], 'draft_pass')} of the {m['drafts_tried']} blocks where it tried a draft committed "
        "as its own draft.",
        "",
        "| Metric | Value | Target (LLD 15, 1) | |",
        "| --- | --- | --- | --- |",
    ]
    for key, label, target, kind in TARGETS:
        t = f"{'≥' if kind == 'min' else '≤'} {fmt(float(target), key) if isinstance(target, float) else f'{target:,}'}"
        if key.startswith("latency") and not m.get("latency_measured", True):
            why = ("rate-limit waits are in the job times; see Model calls for model latency"
                   if (meta.get("rate_limit_waits") or {}).get("waits") else "no model time in this run")
            out.append(f"| {label} | not measured ({why}) | {t} | – |")
        elif key.startswith("tokens") and m.get("tokens_estimated"):
            out.append(f"| {label} | {fmt(m[key], key)} (estimated: characters / 4) | {t} | – |")
        else:
            out.append(f"| {label} | {fmt(m[key], key)} | {t} | {meets(m[key], target, kind)} |")
    out += [
        "",
        "## Jobs",
        "",
        f"- Done: {m['done']} of {m['in_scope']} in-scope prompts ({fmt(m['done_rate'], 'done_rate')}); "
        f"failed: {', '.join(f'{k} {v}' for k, v in sorted(m['failed'].items())) or 'none'}",
        f"- Out of scope refused (`unsupported_request`): {m['out_of_scope_refused']} of {m['out_of_scope']}",
        f"- Plans that use the expected templates: {fmt(m['plan_match'], 'plan_match')}; "
        f"model calls per plan: {fmt(m['plan_rounds_mean'])}",
        f"- Blocks: {m['blocks']}; the model's own draft: {fmt(m['draft_share'], 'draft_share')}; "
        f"spec checks passed: {fmt(m['checks_passed'], 'checks_passed')} of {m['checks']}",
        f"- Assembled circuits: {', '.join(f'{k} {v}' for k, v in m.get('assembly_outcomes', {}).items()) or 'not measured'}; "
        f"repair attempts: {m.get('assembly_repairs', 0)}",
        f"- Fallbacks by cause: {', '.join(f'{k} {v}' for k, v in sorted(m['fallback_why'].items())) or 'none'}",
        f"- Failed attempts by error: "
        f"{', '.join(f'{k} {v}' for k, v in Counter(m['attempt_errors']).most_common()) or 'none'}",
        f"- Tokens per circuit, p95: {fmt(m['tokens_in_p95'], 'tokens')} in, {fmt(m['tokens_out_p95'], 'tokens')} out"
        + (" (estimated: characters / 4)" if m.get("tokens_estimated") else ""),
        "",
        "## By role and level",
        "",
        "| | Blocks | Without fallback | First attempt | Mean attempts |",
        "| --- | --- | --- | --- | --- |",
    ]
    for group in ("by_role", "by_level"):
        for name, s in m[group].items():
            out.append(f"| {name} | {s['blocks']} | {fmt(s['commit_without_fallback'], 'commit_without_fallback')} | "
                       f"{fmt(s['first_attempt_pass'], 'first_attempt_pass')} | {fmt(s['mean_attempts'])} |")
    if m.get("calls"):
        out += ["", "## Model calls", "", "| Kind | Calls | Errors | In (mean) | Out (mean) | Cached in | p50 ms | p95 ms | Models |",
                "| --- | --- | --- | --- | --- | --- | --- | --- | --- |"]
        for kind, s in m["calls"].items():
            errs = ", ".join(f"{k} {v}" for k, v in s["errors"].items()) or "0"
            out.append(f"| {kind} | {s['calls']} | {errs} | {fmt(s['in_tokens_mean'], 'tokens')} | "
                       f"{fmt(s['out_tokens_mean'], 'tokens')} | "
                       f"{s['cached_tokens']:,} | {fmt(s['ms_p50'])} | {fmt(s['ms_p95'])} | {', '.join(s['models'])} |")

    failed = [r for r in rows if in_scope(r) and r["state"] != "done"
              and "AwaitingReply" not in ((r["error"] or {}).get("message") or "")]
    wrong_scope = [r for r in rows if not in_scope(r) and (r["error"] or {}).get("code") != "unsupported_request"]
    fallbacks = [(r, b) for r in rows for b in r["blocks"] if b.get("how") == "fallback"]
    mismatched = [r for r in rows if in_scope(r) and r["plan"] and not plan_matches(r["expect"], r["plan"]["templates"])]
    if failed:
        out += ["", "## Failed jobs", "", "| Prompt | Code | Message |", "| --- | --- | --- |"]
        for r in failed:
            e = r["error"] or {"code": r["state"], "message": ""}
            out.append(f"| {r['id']} | {e['code']} | {cell(e.get('message') or '')} |")
    if wrong_scope:
        out += ["", "## Out-of-scope requests not refused", "", "| Prompt | Outcome | Planned |", "| --- | --- | --- |"]
        for r in wrong_scope:
            outcome = (r["error"] or {}).get("code") or r["state"]
            out.append(f"| {r['id']} | {outcome} | {', '.join(r['plan']['templates']) if r['plan'] else ''} |")
    if fallbacks:
        out += ["", "## Fallbacks", "", "| Prompt | Block | Template | Errors per attempt | Cause |", "| --- | --- | --- | --- | --- |"]
        for r, b in fallbacks:
            errs = "; ".join(",".join(codes) for codes in b.get("errors") or [])
            out.append(f"| {r['id']} | {b['id']} | {b['template']} | {errs} | {b.get('why') or 'attempts'} |")
    if mismatched:
        out += ["", "## Plans without the expected templates", "", "| Prompt | Expected | Planned |", "| --- | --- | --- |"]
        for r in mismatched:
            exp = ", ".join(e if isinstance(e, str) else "/".join(e) for e in r["expect"])
            out.append(f"| {r['id']} | {exp} | {', '.join(r['plan']['templates'])} |")
    return "\n".join(out) + "\n"


def cell(text: str) -> str:
    return text.replace("|", "\\|").replace("\n", " ")[:200]
