"""The metrics, the gate and the report on hand-made rows (no jobs run)."""

from __future__ import annotations

from typing import Any

import pytest

from metrics import gate, markdown, percentile, plan_matches, summarize


def block(how: str, attempts: int, role: str = "filter", errors=(), why=None, checks=()) -> dict[str, Any]:
    return {"id": "b1", "template": "rc_lowpass", "role": role, "how": how, "attempts": attempts,
            "errors": list(errors), "why": why, "checks": list(checks), "attempt_ms": []}


def row(id: str, blocks=(), *, state="done", expect=("rc_lowpass",), plan=("rc_lowpass",), level="beginner",
        tokens=(30_000, 4_000), times=(2.0, 4.0, 10.0), error=None) -> dict[str, Any]:
    return {
        "id": id, "prompt": id, "level": level, "expect": list(expect) if expect != "unsupported" else expect,
        "tags": [], "job_id": None, "state": state, "error": error,
        "plan": {"rounds": 1, "templates": list(plan)} if plan is not None else None,
        "blocks": list(blocks), "tokens": {"in": tokens[0], "out": tokens[1]}, "model": "m",
        "times": dict(zip(("first_narration", "first_op", "total"), times)),
    }


def check(target: float, measured: float | None, ok: bool = True) -> dict[str, Any]:
    return {"name": "fc_hz", "target": target, "measured": measured, "pass": ok}


ROWS = [
    row("a", [block("draft", 1, checks=[check(1000, 1050)])], times=(1.0, 3.0, 8.0)),
    row("b", [block("draft", 2, errors=[["pin_not_found"]], checks=[check(2000, 1900)])], level="advanced",
        tokens=(36_000, 5_000)),
    row("c", [block("fallback", 3, role="amplifier", errors=[["spec_miss"]] * 3, checks=[check(10, None, False)])],
        plan=("sine_source",), times=(1.5, 5.0, 20.0)),
    row("d", [block("use_template", 1)], state="failed", error={"code": "job_timeout", "message": "slow"}),
    row("e", [], state="failed", expect="unsupported", plan=None,
        error={"code": "unsupported_request", "message": "no"}),
]


def test_plan_matches_needs_a_block_per_expected_item():
    assert plan_matches(["sine_source", "rc_lowpass"], ["rc_lowpass", "sine_source", "voltage_follower"])
    assert plan_matches([["voltage_follower", "emitter_follower"]], ["emitter_follower"])
    assert not plan_matches(["noninverting_amp", "noninverting_amp"], ["noninverting_amp"])
    # The second item may only take what the first leaves: a greedy match would fail this one.
    assert plan_matches([["comparator", "schmitt_trigger"], "comparator"], ["comparator", "schmitt_trigger"])


def test_percentile_is_nearest_rank():
    assert percentile([], 95) is None
    assert percentile([5.0], 95) == 5.0
    assert percentile([float(i) for i in range(1, 101)], 95) == 95.0
    assert percentile([1.0, 2.0, 3.0], 50) == 2.0


def test_summary():
    m = summarize(ROWS)
    assert (m["prompts"], m["in_scope"], m["done"], m["out_of_scope"], m["out_of_scope_refused"]) == (5, 4, 3, 1, 1)
    assert m["failed"] == {"job_timeout": 1}
    # Blocks: 2 drafts and a kept template commit without fallback; one fell back.
    assert m["blocks"] == 4
    assert m["commit_without_fallback"] == pytest.approx(3 / 4)
    assert m["draft_share"] == pytest.approx(2 / 4)
    # a and b passed as drafts; c tried three times and fell back; d kept its template without trying.
    assert m["drafts_tried"] == 3 and m["draft_pass"] == pytest.approx(2 / 3)
    assert m["first_attempt_pass"] == pytest.approx(2 / 4)
    assert m["mean_attempts"] == pytest.approx(7 / 4)
    assert m["fallback_why"] == {"attempts": 1} and m["attempt_errors"] == {"pin_not_found": 1, "spec_miss": 3}
    assert m["plan_match"] == pytest.approx(3 / 4)  # c planned the wrong template
    assert m["checks"] == 3 and m["checks_passed"] == pytest.approx(2 / 3)
    assert m["spec_error"] == pytest.approx((0.05 + 0.05) / 2)  # an unmeasured check has no error to average
    # Tokens and latency over finished in-scope jobs only.
    assert m["tokens_in_mean"] == pytest.approx(32_000) and m["tokens_out_p95"] == 5_000
    assert m["latency_total_p95"] == 20.0 and m["latency_first_narration_p95"] == 2.0
    assert m["by_role"]["amplifier"]["commit_without_fallback"] == 0
    assert m["by_level"]["advanced"]["blocks"] == 1 and m["by_level"]["beginner"]["blocks"] == 3
    assert "calls" not in m


def test_a_successful_block_does_not_hide_an_assembly_failure():
    rows = [row("loaded", [block("use_template", 1)]) | {
        "verification": {"status": "failed", "attempt": 2},
    }, row("undriven", [block("use_template", 1)]) | {
        "verification": {"status": "incomplete", "attempt": 0},
    }]
    metrics = summarize(rows)
    assert metrics["assembly_outcomes"] == {"failed": 1, "incomplete": 1}
    assert metrics["assembly_pass_rate"] == 0 and metrics["assembly_repairs"] == 2
    assert "1 assembled circuits failed verification" in gate(metrics)


def test_call_stats():
    calls = [
        {"kind": "plan", "model": "big", "ms": 900, "in_tokens": 6000, "out_tokens": 700, "cached_tokens": 5000},
        {"kind": "plan", "model": None, "ms": 30000, "in_tokens": 0, "out_tokens": 0, "cached_tokens": 0, "error": "timeout"},
        {"kind": "compose", "model": "big", "ms": 1200, "in_tokens": 6100, "out_tokens": 500, "cached_tokens": 5100},
    ]
    s = summarize(ROWS, calls)["calls"]
    assert s["plan"] == {"calls": 2, "errors": {"timeout": 1}, "in_tokens": 6000, "out_tokens": 700,
                         "cached_tokens": 5000, "in_tokens_mean": 6000, "out_tokens_mean": 700, "ms_p50": 900,
                         "ms_p95": 900, "models": ["big"]}
    assert s["compose"]["calls"] == 1


def test_gate_floor_and_regressions():
    m = summarize(ROWS)  # 75% without fallback
    assert gate(m, min_commit=0.75) == []
    (problem,) = gate(m)
    assert "75.0%, below 80%" in problem
    base = m | {"commit_without_fallback": 0.78, "tokens_in_mean": 32_000 / 1.2, "tokens_out_mean": 4_333.0}
    problems = gate(m, base, min_commit=0.5)
    assert len(problems) == 2
    assert "commit rate dropped 3.0 points from 78.0% (at most 2)" in problems[0]
    assert "input tokens per circuit rose 20%" in problems[1]
    # Within the allowances: 2 points and 15%.
    assert gate(m, m | {"commit_without_fallback": 0.765, "tokens_in_mean": 28_000}, min_commit=0.5) == []


def test_a_cassette_miss_fails_the_gate():
    miss = row("f", [], state="failed", error={"code": "internal", "message": "CassetteMiss: no recorded plan reply"})
    m = summarize(ROWS + [miss])
    assert m["cassette_misses"] == {"prompts": ["f"], "calls": 0}
    assert "not in the cassette (f)" in gate(m, min_commit=0)[0]
    # A narration miss fails no job; the call log still shows it.
    m = summarize(ROWS, [{"kind": "narrate", "ms": 1, "in_tokens": 0, "out_tokens": 0, "cached_tokens": 0,
                          "error": "CassetteMiss"}])
    assert "1 model calls are not in the cassette" in gate(m, min_commit=0)[0]


def test_report():
    m = summarize(ROWS)
    text = markdown({"meta": {"started": "2025-12-25T10:00:00+00:00", "provider": "fake", "mode": "compose",
                              "registry_version": "2025.09.0", "wall_s": 12.0},
                     "metrics": m, "gate": gate(m), "rows": ROWS})
    assert "**Gate: failed.**" in text
    assert "| Blocks committed without template fallback | 75.0% | ≥ 90.0% | ❌ |" in text
    assert "| Mean attempts per block | 1.75 | ≤ 1.40 | ❌ |" in text
    assert "the model drafted 50.0% of blocks; 66.7% of the 3 blocks where it tried a draft committed" in text
    assert "| d | job_timeout | slow |" in text
    assert "| c | b1 | rc_lowpass | spec_miss; spec_miss; spec_miss | attempts |" in text
    assert "| c | rc_lowpass | sine_source |" in text
    assert "| Input tokens per circuit (mean) | 32,000 | ≤ 40,000 | ✅ |" in text
    assert "not refused" not in text
    text = markdown({"meta": {"started": "", "provider": "", "mode": "", "registry_version": "", "wall_s": 0},
                     "metrics": m, "gate": None, "rows": [row("e", [], expect="unsupported")]})
    assert "| e | done | rc_lowpass |" in text.split("## Out-of-scope requests not refused")[1]
