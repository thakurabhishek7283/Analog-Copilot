"""The tutor eval metrics on hand-written answers: quantities, numeric grounding, shown arithmetic,
predictions against re-simulated checks, the summary, the gate and the reviewer agreement."""

from __future__ import annotations

import json
import math
from pathlib import Path

from pytest import approx

from tutor_metrics import (agreement, compare, evaluate, gate, ground, markdown, prediction, quantities, read_row,
                           review_rows, summarize)

CONTEXT = """SIM: ok
BLOCK b2 "Sallen-Key low-pass (2nd order)" template sallen_key_lp, filter, committed
  spec: fc_hz 1kHz ±10% measured 996Hz pass; q 0.707 ±15% measured 0.706 pass
PARTS:
  R1 resistor_th "Through-hole resistor, 1/4 W" [b2] resistance=18kΩ 1:B1_OUT 2:B2_N_A | I=-65nA into 1
  C1 cap_film "Film capacitor (non-polarized)" [b2] capacitance=10nF 1:B2_N_A 2:B2_OUT
  U1 opamp_tl072 "TL072 dual JFET op-amp" [b2] OUT_A:B2_OUT
NETS:
  B2_OUT: C1.2 U1.OUT_A | op 2.4mV | ac 705mV -90.1° @1kHz | tran -710mV..700mV"""


def texts(s: str) -> list[tuple[str, float, str]]:
    return [(q.text, q.value, q.unit) for q in quantities(s)]


def test_quantities_read_prefixes_units_and_skip_identifiers():
    assert texts("18kΩ, 10 nF, 0.7 V, −3 dB, 45°, 4.7n and 1,000 Hz") == [
        ("18kΩ", 18e3, "ohm"), ("10 nF", approx(10e-9), "F"), ("0.7 V", 0.7, "V"), ("−3 dB", -3.0, "dB"),
        ("45°", 45.0, "deg"), ("4.7n", approx(4.7e-9), ""), ("1,000 Hz", 1000.0, "Hz")]
    assert texts("R1, B2_OUT, TL072, 2N3904, a 2nd-order filter, ERC010") == []
    assert texts("tran -710mV..700mV") == [("-710mV", approx(-0.71), "V"), ("700mV", approx(0.7), "V")]
    assert texts("10 meters") == [("10", 10.0, "")], "a word after a number is not a prefix"


def test_evaluate_reads_arithmetic_as_answers_write_it():
    fc = 1 / (2 * math.pi * 18e3 * 10e-9)
    assert evaluate("1/(2π·18kΩ·10nF)") == approx(fc)
    assert evaluate("1/(2π × 18 kΩ × 10 nF)") == approx(fc)
    assert evaluate("996 × √18") == approx(996 * math.sqrt(18))
    assert evaluate("1 + 100k/10k") == approx(11)
    assert evaluate("11 × 100 mV") == approx(1.1)
    assert evaluate("1/(2π·R·C)") is None, "symbolic"
    assert evaluate("1/0") is None


def test_grounding_against_the_context_with_shown_arithmetic():
    body = ("[R1] (18 kΩ) and [C1] give fc = 1/(2π·18kΩ·10nF) ≈ 884 Hz; the simulation measured 996 Hz. "
            "The output is 0.7 V, a 2nd-order −40 dB/decade roll-off from the TL072. In general fc = 1/(2π·R·C) ≈ 950 Hz. "
            "Doubling it: 996 Hz × 2 ≈ 2 kHz. Wrong: 18k × 2 = 50k. It draws 123 mA and moves 7.3 kHz.")
    g = ground(body, CONTEXT, "Why is R1 18k?")
    bad = {u["text"]: u["why"] for u in g.ungrounded}
    assert set(bad) == {"50k", "123 mA", "7.3 kHz"}
    assert bad["50k"] == "arithmetic: 18k × 2 = 36k, not 50k"
    assert bad["123 mA"] == bad["7.3 kHz"] == "not in the context"
    assert (g.derived, g.checked) == (3, 2), "884 Hz and 2 kHz checked, 950 Hz after a symbolic formula"
    assert g.grounded == g.quantities - 3


def test_grounding_precision_rounds_by_at_most_five_percent():
    assert not ground("about 1 kHz", "measured 996Hz").ungrounded
    assert not ground("0.7 V", "ac 705mV").ungrounded
    assert ground("1 kHz", "measured 1.4kHz").ungrounded, "1 kHz is not 1.4 kHz rounded"
    assert not ground("a 555 timer", "U1 timer_ne555 NE555").ungrounded, "a name the context uses"
    assert not ground("references are [R10] and [net:B10_X]", "").quantities


def test_what_answers_write_besides_plain_quantities():
    ctx = "spec: fc_hz 1kHz ±10% measured 996Hz → 956Hz | R1 18kΩ C1 10nF | VIN 12V | B1_OUT op 5.17V | I=0.0132fA"
    assert not ground("The band is 900 Hz to 1100 Hz.", ctx).ungrounded, "the edges of a ±10% target"
    assert not ground("R1 drops 6.83 V (12 V − 5.17 V).", ctx).ungrounded, "arithmetic after the result"
    assert not ground("Its input current is 0.0132 fA.", ctx).ungrounded, "femto"
    assert not ground(r"So \(f_c \approx \frac{1}{2\pi \cdot 18\,k\Omega \cdot 10\,nF} \approx 884\,Hz\).", ctx).ungrounded, "LaTeX"
    assert not ground("Set R1 to 36 kΩ.", ctx, suggested=["36k"]).ungrounded, "the experiment's own value"
    assert not ground("In kΩ: 9 × 4.9/(39 + 4.9) ≈ 1.0", "R1 39kΩ R2 4.9kΩ VCC 9V").ungrounded, "a bare 39 for 39kΩ"
    assert not ground("R×C = 18kΩ×10nF. Putting that into 1/(2πRC) gives about 884 Hz.", ctx).ungrounded, "arithmetic in words"
    assert not ground("It covers the cutoff by about 49 times (100 kHz ÷ 2.02 kHz).",
                      "ac 10Hz..100kHz | fc_hz 2kHz measured 2.02kHz").ungrounded, "a ratio's arithmetic"
    assert ground("The cutoff gives about 884 Hz.", ctx).ungrounded, "words without arithmetic are no arithmetic"
    g = ground("fc fell by 40 Hz, then by 282 Hz.", "measured 996Hz → 956Hz → 718Hz")
    assert [(u["text"], u["why"]) for u in g.ungrounded] == [
        ("40 Hz", "one step from the context, arithmetic not shown"), ("282 Hz", "not in the context")
    ], "996 − 956 is one step; 996 − 718 is 278, not 282"
    g = ground("fc fell by 40 Hz.", "measured 996Hz → 956Hz")
    assert g.ungrounded == [{"text": "40 Hz", "why": "one step from the context, arithmetic not shown"}]


def test_a_prediction_is_compared_with_the_check_that_moved():
    # The same cases run in the editor (apps/web/src/tutor/predict.test.ts): one rule, two readings.
    spec = json.loads((Path(__file__).parent / "prediction_cases.json").read_text(encoding="utf-8"))

    def checks(values: dict[str, float | None]) -> list[dict]:
        return [spec["checks"][k] | {"measured": v} for k, v in values.items()]

    for case in spec["cases"]:
        got = prediction(case["predict"], checks(case["before"]), checks(case["after"]))
        assert {k: got[k] for k in case["expect"]} == case["expect"], case["why"]


def answered(rid: str, body: str, *, refs=(("R1", True),), try_=None, tried=None, kind="ask", mode="explain",
             expect=None, tags=(), rubric=None, out_tokens=100) -> dict:
    refs_ = [{"kind": "part", "id": i, "valid": v, "start": 0, "end": 0} for i, v in refs]
    row = {
        "id": rid, "kind": kind, "category": "why_value", "level": "beginner", "mode": mode, "tags": list(tags),
        "expect": expect or {}, "question": "Why?", "text": body,
        "answer": {"body": body, "refs": refs_, "refs_valid": sum(v for _, v in refs), "refs_invalid": sum(not v for _, v in refs),
                   **({"try": try_} if try_ else {})},
        "usage": {"in_tokens": 1500, "out_tokens": out_tokens}, "times": {"first_token": 1.0, "total": 2.0},
        "context": {"text": CONTEXT, "parts": ["R1", "C1", "U1"], "nets": ["B2_OUT"], "blocks": ["b2"], "tokens": 300},
        **({"tried": tried} if tried else {}),
    }
    row["checks"] = read_row(row)
    if rubric:
        row["rubric"] = rubric
    return row


def test_a_row_reads_refs_citations_grounding_and_the_experiment():
    try_ = {"ops": [], "predict": "fc about 500 Hz", "problems": []}
    tried = {"status": "ok", "before": [{"block": "b2", "name": "fc_hz", "unit": "hertz", "measured": 996.0, "tol_pct": 10}],
             "after": [{"block": "b2", "name": "fc_hz", "unit": "hertz", "measured": 499.0, "tol_pct": 10}]}
    r = answered("a", "[R1] is 18 kΩ; [U2] does not exist; 3 V is a guess. What sets [C9]?",
                 refs=(("R1", True), ("U2", False), ("C9", False)), try_=try_, tried=tried, expect={"cite": ["R1", "C1"], "try": "yes"})
    c = r["checks"]
    assert (c["refs"], c["refs_valid"], c["invalid"]) == (3, 1, ["U2", "C9"])
    assert c["cite_missing"] == ["C1"] and c["asks_question"] and c["try_valid"]
    assert [u["text"] for u in c["ungrounded"]] == ["3 V"]
    assert c["prediction"]["held"] is True


def test_summary_and_gate():
    good = [answered(f"g{i}", "[R1] is 18 kΩ.", tags=("rubric",),
                     rubric={"correct": 5, "grounded": 5, "answers": 5, "level": 4, "mode": 5, "teaching": 4,
                             "verdict": "pass", "notes": ""}) for i in range(99)]
    bad = answered("b", "[R7] is 99 kΩ.", refs=(("R7", False),), kind="what_changed", mode="socratic",
                   rubric={"correct": 1, "grounded": 1, "answers": 3, "level": 3, "mode": 2, "teaching": 2,
                           "verdict": "fail", "notes": "Invents R7."}, tags=("rubric",))
    failed = {"id": "f", "kind": "ask", "category": "role", "level": "advanced", "mode": "explain", "tags": [],
              "error": {"code": "llm_unavailable"}, "answer": None, "usage": None, "times": {"first_token": None, "total": None}}
    m = summarize([*good, bad, failed], [])
    assert (m["cases"], m["answered"], m["refs"], m["refs_invalid"]) == (101, 100, 100, 1)
    assert m["refs_valid"] == approx(0.99) and m["grounded_answers"] == approx(0.99)
    assert m["failed"] == {"llm_unavailable": 1}
    assert m["by_kind"]["what_changed"]["refs_valid"] == 0 and m["socratic_asks_question"] == 0
    assert m["rubric"]["pass"] == approx(0.99) and m["rubric"]["by_kind"]["what_changed"] == 0
    assert gate(m) == [], "99% is the floor; one failed case of 101 is within 2%"

    worse = summarize([*good[:97], bad, bad, failed], [])
    assert any("below 99%" in p for p in gate(worse))
    regressed = gate(m | {"grounded_answers": 0.9, "tokens_out_mean": 200.0}, m | {"tokens_out_mean": 100.0})
    assert len(regressed) == 2 and "grounded answers dropped 9.0 points" in regressed[0] and "output tokens" in regressed[1]
    assert gate(m | {"cassette_misses": 2})[0].startswith("2 model calls are not in the cassette")

    report = {"meta": {"provider": "fake", "started": "now", "git": "x", "registry_version": "r", "tiers": {}, "effort": None,
                       "wall_s": 1}, "metrics": m, "gate": [], "rows": [*good, bad, failed]}
    md = markdown(report)
    assert "**Gate: passed**" in md and "| `b` | R7 |" not in md and "- `b`: R7" in md and "Invents R7." in md
    assert compare([report, report]).count("99.0%") >= 4
    sheet = review_rows([*good[:2], bad])
    assert [s["id"] for s in sheet] == ["g0", "g1", "b"] and sheet[2]["judge_verdict"] == "fail"


def test_agreement_with_a_reviewer():
    rows = [{"judge_verdict": "pass", "human_verdict": "pass", "judge_correct": "5", "human_correct": "4"},
            {"judge_verdict": "pass", "human_verdict": "fail", "judge_correct": "4", "human_correct": "1"},
            {"judge_verdict": "fail", "human_verdict": ""}]
    a = agreement(rows)
    assert (a["reviewed"], a["verdict_agreement"]) == (2, 0.5)
    assert a["correct"] == {"n": 2, "mean_abs_diff": 2.0, "within_one": 0.5}


def test_parallel_resistors_and_values_derived_earlier():
    ctx = "R1 10kΩ R2 4.7kΩ VIN 9V"
    assert not ground("4.7kΩ ∥ 1kΩ ≈ 825Ω, so 9V × 825Ω ÷ (10kΩ + 825Ω) ≈ 0.69V.", ctx, "a 1k load").ungrounded
    assert ground("from 4.7 kΩ to about 825 Ω", ctx, "a 1k load").ungrounded == [
        {"text": "825 Ω", "why": "one step from the context, arithmetic not shown"}]
