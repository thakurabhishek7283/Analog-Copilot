"""The tutor eval metrics (LLD §9, §15), the gate and the report, from the rows run_tutor_evals.py
collects: one row per case, holding the answer as the server read it (circuit-core's `Answer`), the
context the model was given, and what re-simulating its `try` block measured. Pure functions, no I/O,
except the two commands at the end.

    python evals/tutor_metrics.py compare DIR [DIR ...]     # runs side by side (tiers, efforts)
    python evals/tutor_metrics.py agreement review.csv      # a reviewer's scores against the judge's

Numeric grounding (LLD §9, rule 2: "use only numbers present in the context, or show the arithmetic
that derives them") is checked on every quantity in the answer's body (its `try` block is left out;
the prediction is checked against the re-simulation instead). A quantity is grounded when:

- it is a value of the context or of the question, at the precision the answer shows it, rounded by
  at most 5% (0.7 V for 705mV, 1 kHz for 996Hz, but not 1 kHz for 1.4kHz);
- it is the result of arithmetic the answer shows (`= ...` or `≈ ...` after an expression with an
  operator). Where the expression is numeric it is evaluated, and a result more than 3% off (and
  outside the precision shown) is an arithmetic error, not grounded. A symbolic one (1/(2π·R·C)) is
  counted as shown;
- it is a textbook constant: a count from 0 to 10, 2π, √2, 0.707, 0.5, −3 dB, 20 or 40 dB per decade,
  0°, 45°, 90°, 180°, a 0.6–0.7 V junction drop, 63% (one time constant);
- it is part of a name the context uses (the 555 of NE555, the 072 of TL072).

Numbers inside identifiers (R1, B2_OUT, 2nd) are not quantities. The checker is deliberately strict
and lists every ungrounded quantity in the report, so a reviewer can tell a model's guess from the
checker's mistake.
"""

from __future__ import annotations

import ast
import csv
import json
import math
import operator
import re
import sys
from collections import Counter
from collections.abc import Iterable
from dataclasses import dataclass
from pathlib import Path
from typing import Any

from metrics import mean, percentile, ratio

# LLD §16, Phase 3 gate: references that name something in the circuit.
PHASE3_REFS = 0.99
# Regression rules against an accepted run (evals/tutor_golden/report.json).
MAX_REFS_DROP = 0.01
MAX_GROUNDED_DROP = 0.03
MAX_TRY_VALID_DROP = 0.05
MAX_TOKEN_RISE = 0.15
MAX_FAILED = 0.02  # cases that got no answer (a provider that kept failing)
WORD_LIMIT = 150  # LLD §9, rule 4

TARGETS = [
    ("refs_valid", "References that name something in the circuit", PHASE3_REFS, "min"),
    ("grounded_answers", "Answers with every number grounded", None, "min"),
    ("grounded_quantities", "Quantities grounded", None, "min"),
    ("try_valid", "Suggested experiments that apply cleanly", None, "min"),
    ("prediction_held", "Predictions within 10% of the re-simulation", None, "min"),
    ("rubric_pass", "Rubric sample judged pass", None, "min"),
]

# ---------------------------------------------------------------- quantities

PREFIX = {"f": 1e-15, "p": 1e-12, "n": 1e-9, "u": 1e-6, "µ": 1e-6, "μ": 1e-6, "m": 1e-3, "": 1.0, "k": 1e3, "K": 1e3,
          "M": 1e6, "meg": 1e6, "G": 1e9}
UNIT = {"Ω": "ohm", "ohm": "ohm", "ohms": "ohm", "R": "ohm", "F": "F", "H": "H", "Hz": "Hz", "V": "V", "A": "A",
        "W": "W", "s": "s", "%": "%", "dB": "dB", "°": "deg", "deg": "deg", "degrees": "deg", "V/V": ""}

_NUM = r"(?P<num>[-−+]?\d+(?:[.,]\d+)?(?:[eE][-+]?\d+)?)"
_PRE = r"(?P<pre>meg|[fpnuµμmkKMG])?"
_UNIT = r"(?P<unit>V/V|Hz|dB|Ω|ohms?\b|degrees\b|deg\b|°|%|[FHVAWs](?![A-Za-z]))?"
QUANTITY = re.compile(
    r"(?<![\w\]])(?<!\d\.)" + _NUM + r"(?:\s?" + _PRE + _UNIT + r")"
    r"(?![\w])"
)
REF = re.compile(r"\[(?:net:|block:)?[A-Za-z][A-Za-z0-9_]*\]")
IDENT = re.compile(r"\b[A-Za-z_]*\d+[A-Za-z_][A-Za-z0-9_]*\b|\b[A-Za-z_]+\d+\b")


@dataclass(frozen=True)
class Quantity:
    text: str
    value: float  # SI, signed
    unit: str  # family: ohm, F, H, Hz, V, A, W, s, %, dB, deg, or "" (none)
    digits: float  # half a step of the last digit shown, in SI: the precision the answer claims
    start: int
    end: int
    shown: float  # the number as written, without its prefix (39 for 39kΩ)


def _number(raw: str) -> float:
    return float(raw.replace("−", "-").replace(",", ""))


def quantities(text: str) -> list[Quantity]:
    """Every quantity in `text`: a number with an optional SI prefix and unit (18kΩ, 0.7 V, 4.7n,
    -3 dB, 45°). Numbers that are part of an identifier (R1, B2_OUT, TL072, 2nd) are skipped."""
    out = []
    for m in QUANTITY.finditer(text):
        num, pre, unit = m.group("num"), m.group("pre") or "", m.group("unit") or ""
        if "," in num and not re.fullmatch(r"[-−+]?\d{1,3}(,\d{3})+", num):
            continue  # "1,2" is two numbers in a list
        if pre and not unit and m.group(0)[len(num):].startswith(" "):
            continue  # "18 k..." is a word, not a prefix
        frac = num.replace("−", "-").lstrip("+-").split("e")[0].split("E")[0]
        decimals = len(frac.split(".")[1]) if "." in frac else 0
        scale = PREFIX[pre]
        value = _number(num) * scale
        out.append(Quantity(m.group(0).strip(), value, UNIT.get(unit, ""), 0.5 * 10 ** -decimals * scale,
                            m.start(), m.end(), _number(num)))
    return out


LATEX = [(r"\\times", "×"), (r"\\cdot", "·"), (r"\\approx", "≈"), (r"\\pi\b", "π"), (r"\\Omega", "Ω"),
         (r"\\mu", "µ"), (r"\\pm", "±"), (r"\^\{?\\circ\}?", "°"), (r"\\circ", "°"), (r"\\%", "%"),
         (r"\\parallel", "∥"), (r"\\[,;:! ]", " "), (r"\\(?:left|right)", ""), (r"\\[()\[\]]", " "), (r"\$", " ")]


def plain(text: str) -> str:
    r"""LaTeX an answer slipped into (`\(9\times4.9\)`, `\frac{a}{b}`) as the plain text it means, so
    its numbers and arithmetic read like any other."""
    if "\\" not in text and "$" not in text:
        return text
    s = text
    for _ in range(3):  # nested \frac and \sqrt, innermost first
        s = re.sub(r"\\[dt]?frac\{([^{}]*)\}\{([^{}]*)\}", r"(\1)/(\2)", s)
        s = re.sub(r"\\sqrt\{([^{}]*)\}", r"√(\1)", s)
        s = re.sub(r"\\(?:text|mathrm|operatorname|mathbf)\{([^{}]*)\}", r"\1", s)
    for pattern, repl in LATEX:
        s = re.sub(pattern, repl, s)
    return s.replace("{", "").replace("}", "")


def names(text: str) -> set[str]:
    """Digit runs inside the identifiers a text uses (NE555 → 555, TL072 → 072 and 72, LM7805 → 7805)."""
    out = set()
    for ident in IDENT.findall(text):
        for run in re.findall(r"\d+", ident):
            out.update({run, run.lstrip("0") or "0"})
    return out


def _close(q: Quantity, value: float, rel: float = 0.01) -> bool:
    """`value` written as `q` at the precision `q` shows, rounding by at most 5% (0.7 V for 705mV,
    1 kHz for 996Hz, not for 1.4kHz), or within `rel`."""
    a, b = abs(q.value), abs(value)
    return abs(a - b) <= max(min(q.digits * 1.0001, 0.05 * a), rel * max(a, b), 1e-15)


CONSTANTS: list[tuple[float, str]] = [
    (2 * math.pi, ""), (math.pi, ""), (math.sqrt(2), ""), (1 / math.sqrt(2), ""), (0.5, ""), (0.25, ""),
    (3, "dB"), (6, "dB"), (20, "dB"), (40, "dB"), (60, "dB"),
    (0, "deg"), (45, "deg"), (90, "deg"), (180, "deg"), (270, "deg"), (360, "deg"),
    (0.6, "V"), (0.65, "V"), (0.7, "V"), (0.025, "V"), (0.026, "V"),  # junction drops, the thermal voltage
    (63, "%"), (37, "%"), (50, "%"), (100, "%"),
]
# What one step of arithmetic may multiply or divide by and still count as "one step from the
# context": not grounded (the rule asks for the arithmetic shown), but reported apart from numbers
# from nowhere.
FACTORS = [2, 3, 4, 5, 10, 0.5, 0.1, 2 * math.pi, math.sqrt(2)]


def constant(q: Quantity) -> bool:
    if q.unit == "" and float(q.value).is_integer() and 0 <= abs(q.value) <= 10:
        return True
    return any(q.unit in (unit, "") and _close(q, c) for c, unit in CONSTANTS if unit or q.unit == "")


# ---------------------------------------------------------------- arithmetic shown in the answer

_OPS = {ast.Add: operator.add, ast.Sub: operator.sub, ast.Mult: operator.mul, ast.Div: operator.truediv,
        ast.Pow: operator.pow, ast.USub: operator.neg, ast.UAdd: operator.pos}
# The sign before a result, or the words that stand for it ("1/(2πRC) gives about 884 Hz", dry run).
EQUALS = re.compile(r"\s*(=|≈|~|≅|≃|\b(?:gives|giving|comes to|works out (?:to|at)|equals|yields)"
                    r"(?:\s+(?:about|roughly|around|approximately|nearly|close to))?)\s*$", re.I)
OPERATOR = re.compile(r"[·×*/÷+^√²∥]|\|\||(?<=[\d)\s])[-−](?=[\s\d(])|\bx\b")


def _eval(node: ast.AST) -> float:
    if isinstance(node, ast.Expression):
        return _eval(node.body)
    if isinstance(node, ast.Constant) and isinstance(node.value, (int, float)):
        return float(node.value)
    if isinstance(node, ast.BinOp) and isinstance(node.op, ast.BitOr):  # a ∥ b: resistors in parallel
        a, b = _eval(node.left), _eval(node.right)
        return a * b / (a + b)
    if isinstance(node, ast.BinOp) and type(node.op) in _OPS:
        return _OPS[type(node.op)](_eval(node.left), _eval(node.right))
    if isinstance(node, ast.UnaryOp) and type(node.op) in _OPS:
        return _OPS[type(node.op)](_eval(node.operand))
    if isinstance(node, ast.Call) and isinstance(node.func, ast.Name) and node.func.id == "sqrt" and len(node.args) == 1:
        return math.sqrt(_eval(node.args[0]))
    if isinstance(node, ast.Name) and node.id == "pi":
        return math.pi
    raise ValueError("not numeric")


def evaluate(expr: str) -> float | None:
    """A numeric expression as an answer writes it (1/(2π·18kΩ·10nF), 11 × 100 mV, √(10n·4.7n)), or
    None when it is symbolic or does not parse."""
    s = expr.strip()
    parts, last = [], 0
    for q in quantities(s):
        parts.append(s[last:q.start])
        parts.append(f"({q.value!r})")
        last = q.end
    parts.append(s[last:])
    s = "".join(parts)
    s = (s.replace("·", "*").replace("×", "*").replace("⋅", "*").replace("÷", "/").replace("−", "-")
         .replace("^", "**").replace("²", "**2").replace("π", "*pi").replace(" x ", "*").replace("∥", "|").replace("||", "|"))
    s = re.sub(r"√\s*(\((?:[^()]|\([^()]*\))*\)|[\d.e+-]+|\([^)]*\))", r"sqrt(\1)", s)
    s = re.sub(r"(^|[(*/+\-])\s*\*pi", r"\1pi", s)  # π after an operator or at the start
    s = re.sub(r"\)\s*\(", ")*(", s)  # implicit multiplication between brackets: 2π(18k)(10n)
    s = re.sub(r"\)\s*(pi|sqrt)", r")*\1", s)
    s = re.sub(r"\bpi\s*\(", "pi*(", s)
    probe = re.sub(r"\d+(?:\.\d*)?(?:[eE][-+]?\d+)?|\b(?:pi|sqrt)\b", "", s)
    if re.search(r"[^\s()+\-*/.|]", probe):
        return None
    try:
        v = _eval(ast.parse(s, mode="eval"))
    except (SyntaxError, ValueError, ZeroDivisionError, OverflowError, TypeError):
        return None
    return v if math.isfinite(v) else None


def shown_arithmetic(text: str, q: Quantity) -> tuple[str, float | None] | None:
    """When `q` follows `=` or `≈` (or "gives about", "comes to") after an expression with an
    operator, that expression and its value: None when it is symbolic (1/(2π·R·C)). Else None."""
    before = text[:q.start]
    sign = EQUALS.search(before)
    if not sign:
        return None
    head = before[:sign.start()]
    cut = max(head.rfind(c) for c in "=≈~≅≃\n:;")
    sentence = max(head.rfind(". "), head.rfind("? "), head.rfind("! "))
    expr = head[max(cut, sentence) + 1:]
    if not OPERATOR.search(expr):
        return None
    # The words before the expression ("so the cutoff is 1/(...)") are not part of it.
    numeric = re.sub(r"^[^\d(√π−-]*", "", expr)
    return expr.strip(), evaluate(numeric) if numeric else None


def trailing_arithmetic(text: str, q: Quantity) -> tuple[str, float | None] | None:
    """When `q` is followed by its arithmetic in brackets ("6.83 V (12 V − 5.17 V)", "49 times
    (100 kHz ÷ 2.02 kHz)"), that expression and its value (None if symbolic)."""
    m = re.match(r"\s*(?:times\s+)?\(((?:[^()]|\([^()]*\))*)\)", text[q.end:])
    if not m or not OPERATOR.search(m.group(1)) or not re.search(r"\d", m.group(1)):
        return None
    return m.group(1).strip(), evaluate(m.group(1))


def known_values(context: str, question: str, suggested: Iterable[str] = ()) -> list[tuple[float, str, float]]:
    """(value, unit, as written) for every quantity the answer may use as given: the context's, the
    question's, the edges of each `target ±tol%`, and the values its own experiment sets."""
    out = [(k.value, k.unit, k.shown) for k in quantities(strip_refs(context)) + quantities(question)]
    for m in re.finditer(r"(\S+)\s*±\s*(\d+(?:\.\d+)?)%", context):
        for k in quantities(m.group(1)):
            p = float(m.group(2)) / 100
            out += [(k.value * (1 - p), k.unit, k.shown * (1 - p)), (k.value * (1 + p), k.unit, k.shown * (1 + p))]
    for s in suggested:
        out += [(k.value, k.unit, k.shown) for k in quantities(str(s))]
    return out


def _same(q: Quantity, unit: str) -> bool:
    return q.unit == unit or "" in (q.unit, unit)


def one_step(q: Quantity, known: list[tuple[float, str, float]]) -> bool:
    """`q` is one step of arithmetic from what the answer was given: a sum or difference of two
    values in the same unit (or two resistances in parallel), a value times or over a small factor (2, 10, ½, 2π...) or a ratio the
    context gives, or Ohm's law."""
    units = [(v, u) for v, u, _ in known]
    ratios = [v for v, u in units if u == "" and v] + FACTORS
    for i, (a, ua) in enumerate(units):
        if ua and _same(q, ua):
            if any((ub == ua and (_close(q, a + b) or _close(q, a - b)))
                   or (ua == "ohm" and ub in ("ohm", "") and a + b and _close(q, a * b / (a + b)))  # in parallel
                   for j, (b, ub) in enumerate(units) if j != i):
                return True
            if any(_close(q, a * f) or _close(q, a / f) for f in ratios):
                return True
        if not ua and q.unit == "" and any(b and _close(q, a / b) for b, ub in units if not ub):
            return True
    law = {"A": ("V", "ohm"), "ohm": ("V", "A")}
    if q.unit in law:
        num, den = law[q.unit]
        return any(b and _close(q, a / b) for a, ua in units if ua == num for b, ub in units if ub == den)
    if q.unit in ("V", "W"):
        x, y = ("A", "ohm") if q.unit == "V" else ("V", "A")
        return any(_close(q, a * b) for a, ua in units if ua == x for b, ub in units if ub == y)
    return False


# ---------------------------------------------------------------- grounding

@dataclass
class Grounding:
    quantities: int
    grounded: int
    derived: int  # results of shown arithmetic, checked or symbolic
    checked: int  # of which the arithmetic was evaluated and agreed
    ungrounded: list[dict[str, Any]]  # {text, why}: why is "arithmetic: ...", ONE_STEP or NOWHERE


ONE_STEP = "one step from the context, arithmetic not shown"
NOWHERE = "not in the context"


def strip_refs(text: str) -> str:
    """References replaced by spaces of the same length (so offsets still hold)."""
    return REF.sub(lambda m: " " * len(m.group(0)), text)


def ground(body: str, context: str, question: str = "", suggested: Iterable[str] = ()) -> Grounding:
    """Every quantity in an answer's body against the context it was given, the question, and the
    values its own experiment sets (`suggested`: they are the experiment, checked by simulating it)."""
    text = strip_refs(plain(body))
    known = known_values(context, question, suggested)
    ident = names(context) | names(question)
    qs = quantities(text)
    grounded = derived = checked = 0
    bad = []
    for q in qs:
        bare = re.sub(r"^[-−+]", "", q.text)
        if any(_close(q, v) and _same(q, u) for v, u, _ in known):
            grounded += 1
            continue
        if constant(q) or (q.unit == "" and (bare in ident or any(_close(q, s) for _, _, s in known))):
            grounded += 1  # also a bare 39 for 39kΩ, written in kΩ
            continue
        shown = shown_arithmetic(text, q) or trailing_arithmetic(text, q)
        if shown is not None:
            expr, value = shown
            if value is None:
                grounded += 1
                derived += 1
                known.append((q.value, q.unit, q.shown))
                continue
            if _close(q, value, rel=0.03):
                grounded += 1
                derived += 1
                checked += 1
                known.append((q.value, q.unit, q.shown))  # later steps may use it
                continue
            bad.append({"text": q.text, "why": f"arithmetic: {expr} = {fmt(value)}, not {q.text}"})
            continue
        bad.append({"text": q.text, "why": ONE_STEP if one_step(q, known) else NOWHERE})
    return Grounding(len(qs), grounded, derived, checked, bad)


def fmt(v: float) -> str:
    if v == 0:
        return "0"
    exp = math.floor(math.log10(abs(v)) / 3) * 3
    exp = max(-12, min(9, exp))
    prefix = {-12: "p", -9: "n", -6: "µ", -3: "m", 0: "", 3: "k", 6: "M", 9: "G"}[exp]
    return f"{v / 10 ** exp:.3g}{prefix}"


# ---------------------------------------------------------------- try blocks

CHECK_UNIT = {"hertz": "Hz", "volt": "V", "ampere": "A", "ohm": "ohm", "unitless": "", "second": "s"}
# A bound, not a value: "well below 2 kHz" is not a prediction of 2 kHz.
BOUND = re.compile(r"(?:\b(?:below|above|under|over|less than|more than|at least|at most|up to)|[<>≤≥])\s*$", re.I)


def prediction(predict: str, before: list[dict[str, Any]], after: list[dict[str, Any]]) -> dict[str, Any]:
    """The `predict` text against the spec checks re-simulated before and after the experiment (the
    caller leaves out stimulus checks, such as a source's amplitude). Each quantity in it is compared
    with every check of its unit that the experiment moved by more than 1%: the prediction held when
    one is within 10% or the check's tolerance, whichever is wider; it missed when there were checks
    to compare and none agreed; with nothing to compare, it is not judged. A number after a bound
    ("below", "more than", "<") is not a value to compare."""
    text = plain(predict)
    qs = [q for q in quantities(text) if not BOUND.search(text[:q.start])]
    out: dict[str, Any] = {"number": qs[0].text if qs else None, "check": None, "measured": None, "held": None}
    was = {(c["block"], c["name"]): c.get("measured") for c in before}

    def moved(c: dict[str, Any]) -> bool:
        b = was.get((c["block"], c["name"]))
        return b is None or abs(c["measured"] - b) > 0.01 * max(abs(b), 1e-12)

    pool = [c for c in after if c.get("measured") is not None and moved(c)]
    pairs = [(q, c) for q in qs for c in pool if CHECK_UNIT.get(c.get("unit"), "") == q.unit]
    if not pairs:
        return out

    def err(pair: tuple[Quantity, dict[str, Any]]) -> float:
        q, c = pair
        return abs(abs(q.value) - abs(c["measured"])) / max(abs(c["measured"]), 1e-12)

    q, best = min(pairs, key=err)
    tol = max(0.10, (best.get("tol_pct") or 0) / 100)
    out.update(number=q.text, check=f"{best['block']} {best['name']}", measured=best["measured"],
               held=err((q, best)) <= tol)
    return out


# ---------------------------------------------------------------- per answer

def _values(body: dict[str, Any]) -> list[str]:
    """The values an experiment's op sets: a parameter, or a new part's parameters."""
    out = [body["value"]] if isinstance(body.get("value"), str) else []
    return out + [v for v in (body.get("params") or {}).values() if isinstance(v, str)]


def words(text: str) -> int:
    return len(re.findall(r"\S+", text))


# What a beginner's answer should say in plain words (the tutor's system prompt, "The student's level").
JARGON = [
    ("ERC", re.compile(r"\bERC\d*\b")),
    ("operating point", re.compile(r"\boperating[- ]point\b", re.I)),
    ("DC bias", re.compile(r"\bDC bias\b", re.I)),
    ("quiescent", re.compile(r"\bquiescent\b", re.I)),
    ("pin name", re.compile(r"(?<!\w)[A-Z]{1,3}\d+\.(?:\d+\b|[A-Z][A-Z0-9_]*\b)")),
    ("LaTeX", re.compile(r"\\[(\[]|\\(?:frac|sqrt|times|cdot|approx|pi|Omega|text)\b|\$[^$\n]+\$")),
]
PHASES = re.compile(r"[-−]?\d+(?:\.\d+)?\s?°")


def jargon(text: str) -> list[str]:
    """The terms and notations a beginner's answer should not use, each once; two or more phase
    angles count as a list of phases."""
    out = [name for name, pattern in JARGON if pattern.search(text)]
    if len(PHASES.findall(text)) >= 2:
        out.append("phase angles")
    return out


def read_row(row: dict[str, Any]) -> dict[str, Any]:
    """The automatic checks on one answered case, added to its row as `checks`."""
    a = row["answer"]
    ctx = row["context"]
    refs = a.get("refs") or []
    listed = set(ctx["parts"]) | {f"net:{n}" for n in ctx["nets"]} | {f"block:{b}" for b in ctx["blocks"]}

    def rid(r: dict[str, Any]) -> str:
        return r["id"] if r["kind"] == "part" else f"{r['kind']}:{r['id']}"

    # A part the explained edit removed is named in the change summary but is not in the circuit after
    # it: the editor shows it as plain text, but it is not an invention, so it counts as valid here.
    removed = [rid(r) for r in refs if not r["valid"] and rid(r) in listed]
    cited = {rid(r) for r in refs if r["valid"]}
    t = a.get("try")
    suggested = [v for op in (t or {}).get("ops") or [] for v in _values(op.get("body") or {})]
    # A follow-up's input holds the conversation so far: its numbers are the model's to use.
    said = [x for turn in row.get("turns") or () for x in (turn["question"], turn["text"])]
    g = ground(a["body"], ctx["text"], "\n".join([*said, row.get("question") or ""]), suggested)
    expect = row.get("expect") or {}
    out: dict[str, Any] = {
        "refs": len(refs), "refs_valid": a["refs_valid"] + len(removed), "refs_invalid": a["refs_invalid"] - len(removed),
        "invalid": [rid(r) for r in refs if not r["valid"] and rid(r) not in listed], "removed": removed,
        "outside_slice": sorted({rid(r) for r in refs if r["valid"]} - listed),
        "cite_expected": expect.get("cite") or [], "cite_missing": [c for c in expect.get("cite") or [] if c not in cited],
        "quantities": g.quantities, "grounded": g.grounded, "derived": g.derived, "checked": g.checked,
        "ungrounded": g.ungrounded, "one_step_only": all(u["why"] == ONE_STEP for u in g.ungrounded),
        "words": words(a["body"]), "asks_question": "?" in a["body"], "follow_up": bool(row.get("turns")),
        "jargon": jargon(a["body"]) if row.get("level") == "beginner" else [],
        "try": t is not None, "try_valid": None if t is None else not t.get("problems"),
        "try_expected": expect.get("try"), "declines_expected": bool(expect.get("declines")),
    }
    tried = row.get("tried")
    if t is not None and tried:
        out["prediction"] = prediction(t.get("predict") or "", tried.get("before") or [], tried.get("after") or [])
        out["tried_status"] = tried.get("status")
    return out


# ---------------------------------------------------------------- the run

def _slice(rows: list[dict[str, Any]]) -> dict[str, Any]:
    done = [r for r in rows if r.get("checks")]
    cs = [r["checks"] for r in done]
    refs = sum(c["refs"] for c in cs)
    tries = [c for c in cs if c["try"]]
    preds = [c["prediction"] for c in tries if c.get("prediction") and c["prediction"]["held"] is not None]
    return {
        "cases": len(rows),
        "answered": len(done),
        "refs": refs,
        "refs_valid": ratio(sum(c["refs_valid"] for c in cs), refs),
        "quantities": sum(c["quantities"] for c in cs),
        "grounded_quantities": ratio(sum(c["grounded"] for c in cs), sum(c["quantities"] for c in cs)),
        "grounded_answers": ratio(sum(1 for c in cs if not c["ungrounded"]), len(cs)),
        "grounded_or_one_step": ratio(sum(1 for c in cs if c["one_step_only"]), len(cs)),
        "tries": len(tries),
        "try_valid": ratio(sum(1 for c in tries if c["try_valid"]), len(tries)),
        "prediction_held": ratio(sum(1 for p in preds if p["held"]), len(preds)),
        "words_mean": mean([c["words"] for c in cs]),
    }


def summarize(rows: list[dict[str, Any]], calls: list[dict[str, Any]] | None = None, *,
              latency_measured: bool = True) -> dict[str, Any]:
    """The metrics of one tutor eval run. `calls`: every model call it made (answers and judge)."""
    done = [r for r in rows if r.get("checks")]
    cs = [r["checks"] for r in done]
    tries = [c for c in cs if c["try"]]
    preds = [c["prediction"] for c in tries if c.get("prediction")]
    expect_yes = [c for c in cs if c["try_expected"] == "yes"]
    expect_no = [c for c in cs if c["try_expected"] == "no"]
    cited = [c for c in cs if c["cite_expected"]]
    # A follow-up may confirm a right reply and stop asking: the judge grades those.
    socratic = [r["checks"] for r in done if r.get("mode") == "socratic" and not r["checks"].get("follow_up")]
    beginner = [r["checks"] for r in done if r.get("level") == "beginner"]
    graded = [r for r in done if r.get("rubric")]
    m: dict[str, Any] = {
        **_slice(rows),
        "failed": dict(Counter((r.get("error") or {}).get("code") or "unknown" for r in rows if not r.get("checks"))),
        "refs_invalid": sum(c["refs_invalid"] for c in cs),
        "answers_with_invalid_refs": sum(1 for c in cs if c["refs_invalid"]),
        "refs_outside_slice": sum(len(c["outside_slice"]) for c in cs),
        "refs_removed": sum(len(c["removed"]) for c in cs),
        "one_step_not_shown": sum(1 for c in cs for u in c["ungrounded"] if u["why"] == ONE_STEP),
        "from_nowhere": sum(1 for c in cs for u in c["ungrounded"] if u["why"] == NOWHERE),
        "cite_met": ratio(sum(1 for c in cited if not c["cite_missing"]), len(cited)),
        "derived": sum(c["derived"] for c in cs),
        "derived_checked": sum(c["checked"] for c in cs),
        "arithmetic_errors": sum(1 for c in cs for u in c["ungrounded"] if u["why"].startswith("arithmetic")),
        "try_share": ratio(len(tries), len(cs)),
        "try_when_expected": ratio(sum(1 for c in expect_yes if c["try"]), len(expect_yes)),
        "no_try_when_not_expected": ratio(sum(1 for c in expect_no if not c["try"]), len(expect_no)),
        "tries_simulated": sum(1 for c in tries if c.get("tried_status") == "ok"),
        "predictions_with_number": sum(1 for p in preds if p["number"]),
        "predictions_compared": sum(1 for p in preds if p["held"] is not None),
        "words_p95": percentile([c["words"] for c in cs], 95),
        "over_word_limit": sum(1 for c in cs if c["words"] > WORD_LIMIT),
        "socratic_asks_question": ratio(sum(1 for c in socratic if c["asks_question"]), len(socratic)),
        "beginner_jargon": ratio(sum(1 for c in beginner if c.get("jargon")), len(beginner)),
        "jargon_terms": dict(Counter(j for c in beginner for j in c.get("jargon") or ())),
        "tokens_in_mean": mean([r["usage"]["in_tokens"] for r in done]),
        "tokens_out_mean": mean([r["usage"]["out_tokens"] for r in done]),
        "tokens_out_p95": percentile([r["usage"]["out_tokens"] for r in done], 95),
        "first_token_p50": percentile([r["times"]["first_token"] for r in done if r["times"]["first_token"] is not None], 50),
        "first_token_p95": percentile([r["times"]["first_token"] for r in done if r["times"]["first_token"] is not None], 95),
        "answer_p50": percentile([r["times"]["total"] for r in done], 50),
        "answer_p95": percentile([r["times"]["total"] for r in done], 95),
        "latency_measured": latency_measured,
        "by_kind": {k: _slice([r for r in rows if r["kind"] == k]) for k in ("ask", "what_changed") if any(r["kind"] == k for r in rows)},
        "by_category": {k: _slice([r for r in rows if r["category"] == k]) for k in sorted({r["category"] for r in rows})},
        "by_level": {k: _slice([r for r in rows if r["level"] == k]) for k in ("beginner", "intermediate", "advanced") if any(r["level"] == k for r in rows)},
        "by_mode": {k: _slice([r for r in rows if r.get("mode") == k]) for k in ("explain", "socratic") if any(r.get("mode") == k for r in rows)},
        "rubric": rubric_stats(graded),
        "cassette_misses": sum(1 for c in calls or () if c.get("error") == "CassetteMiss"),
        "awaiting": sum(1 for c in calls or () if c.get("error") == "AwaitingReply"),
        "tokens_estimated": any(c.get("estimated") for c in calls) if calls else False,
    }
    m["rubric_pass"] = m["rubric"]["pass"] if m["rubric"] else None
    if calls is not None:
        from metrics import call_stats

        m["calls"] = call_stats(calls)
    return m


CRITERIA = ("correct", "grounded", "answers", "level", "mode", "teaching")


def rubric_stats(graded: list[dict[str, Any]]) -> dict[str, Any] | None:
    ok = [r for r in graded if r["rubric"].get("verdict") in ("pass", "fail")]
    if not ok:
        return None
    return {
        "graded": len(ok),
        "pass": ratio(sum(1 for r in ok if r["rubric"]["verdict"] == "pass"), len(ok)),
        "means": {k: mean([r["rubric"][k] for r in ok if isinstance(r["rubric"].get(k), (int, float))]) for k in CRITERIA},
        "by_kind": {k: ratio(sum(1 for r in ok if r["kind"] == k and r["rubric"]["verdict"] == "pass"),
                             sum(1 for r in ok if r["kind"] == k)) for k in ("ask", "what_changed")},
    }


def gate(m: dict[str, Any], baseline: dict[str, Any] | None = None) -> list[str]:
    """Why the run fails the gate; empty when it passes."""
    problems = []
    if m.get("awaiting"):
        problems.append(f"the run is not complete: {m['awaiting']} model calls are waiting for a reply (exchange)")
    if m.get("cassette_misses"):
        problems.append(f"{m['cassette_misses']} model calls are not in the cassette: the prompts, the context builder "
                        "or the fixtures changed since it was recorded; run the live tutor evals again")
    failed = m["cases"] - m["answered"]
    if m["cases"] and failed / m["cases"] > MAX_FAILED:
        problems.append(f"{failed} of {m['cases']} cases got no answer ({m['failed']})")
    r = m["refs_valid"]
    if r is None:
        problems.append("no answer cited anything")
    elif r < PHASE3_REFS:
        problems.append(f"valid references {r:.1%}, below {PHASE3_REFS:.0%} (the Phase 3 gate)")
    if baseline is not None:
        for key, drop, what in (("refs_valid", MAX_REFS_DROP, "valid references"),
                                ("grounded_answers", MAX_GROUNDED_DROP, "grounded answers"),
                                ("try_valid", MAX_TRY_VALID_DROP, "valid experiments")):
            now, then = m.get(key), baseline.get(key)
            if now is not None and then is not None and now < then - drop:
                problems.append(f"{what} dropped {(then - now) * 100:.1f} points from {then:.1%} (at most {drop * 100:.0f})")
        measured = not m.get("tokens_estimated") and not baseline.get("tokens_estimated")
        for key, what in (("tokens_in_mean", "input"), ("tokens_out_mean", "output")) if measured else ():
            now, then = m.get(key), baseline.get(key)
            if now is not None and then and now > then * (1 + MAX_TOKEN_RISE):
                problems.append(f"{what} tokens per answer rose {now / then - 1:.0%} (at most {MAX_TOKEN_RISE:.0%})")
    return problems


# ---------------------------------------------------------------- report

def pct(x: float | None) -> str:
    return "–" if x is None else f"{x:.1%}"


def num(x: float | None, digits: int = 0) -> str:
    return "–" if x is None else f"{x:,.{digits}f}"


def _cell(text: str, limit: int = 90) -> str:
    text = " ".join(str(text).split()).replace("|", "\\|")
    return text if len(text) <= limit else text[:limit - 1] + "…"


def markdown(report: dict[str, Any]) -> str:
    meta, m, rows = report["meta"], report["metrics"], report["rows"]
    out = [f"# Tutor evals: {meta['provider']}", ""]
    out.append(f"{meta['started']} · git {meta.get('git') or '?'} · registry {meta['registry_version']} · "
               f"tiers {meta['tiers']} · effort {meta.get('effort') or 'provider default'} · "
               f"{m['cases']} cases, {m['answered']} answered · {meta['wall_s']} s")
    if report.get("gate") is not None:
        out += ["", "**Gate: passed** ✅" if not report["gate"] else "**Gate: failed** ❌"]
        out += [f"- {p}" for p in report["gate"]]
    out += ["", "| Metric | Value | Target |", "| --- | --- | --- |"]
    for key, label, target, _ in TARGETS:
        out.append(f"| {label} | {pct(m.get(key))} | {'≥ ' + format(target, '.0%') if target else 'report'} |")
    out += [
        "", "| | |", "| --- | --- |",
        f"| References | {m['refs']} ({m['refs_invalid']} invalid in {m['answers_with_invalid_refs']} answers; "
        f"{m['refs_outside_slice']} valid but outside the slice; {m['refs_removed']} to parts the explained edit "
        "removed, counted valid) |",
        f"| Expected citations made | {pct(m['cite_met'])} |",
        f"| Quantities | {m['quantities']} ({m['derived']} derived, {m['derived_checked']} with the arithmetic checked); "
        f"ungrounded: {m['arithmetic_errors']} arithmetic errors, {m['one_step_not_shown']} one step from the context "
        f"without the arithmetic, {m['from_nowhere']} not in the context |",
        f"| Answers grounded, or only one step short | {pct(m['grounded_or_one_step'])} |",
        f"| Experiments | {m['tries']} in {pct(m['try_share'])} of answers; when expected {pct(m['try_when_expected'])}; "
        f"none when not wanted {pct(m['no_try_when_not_expected'])} |",
        f"| Predictions | {m['predictions_with_number']} with a number, {m['predictions_compared']} compared with "
        f"a re-simulated check, {pct(m['prediction_held'])} held |",
        f"| Words | mean {num(m['words_mean'])}, p95 {num(m['words_p95'])}, {m['over_word_limit']} over {WORD_LIMIT} |",
        f"| Socratic answers that ask a question | {pct(m['socratic_asks_question'])} (follow-ups left out) |",
        f"| Beginner answers with jargon | {pct(m.get('beginner_jargon'))}"
        f"{' (' + ', '.join(f'{k} {v}' for k, v in sorted(m['jargon_terms'].items())) + ')' if m.get('jargon_terms') else ''} |",
        f"| Tokens per answer | in {num(m['tokens_in_mean'])}, out {num(m['tokens_out_mean'])} (p95 "
        f"{num(m['tokens_out_p95'])}){' (estimated)' if m['tokens_estimated'] else ''} |",
    ]
    if m["latency_measured"]:
        out.append(f"| Latency | first token p50 {num(m['first_token_p50'], 1)} s, p95 {num(m['first_token_p95'], 1)} s; "
                   f"whole answer p50 {num(m['answer_p50'], 1)} s, p95 {num(m['answer_p95'], 1)} s |")
    rub = m.get("rubric")
    if rub:
        means = ", ".join(f"{k} {num(v, 2)}" for k, v in rub["means"].items())
        out.append(f"| Rubric ({rub['graded']} graded) | {pct(rub['pass'])} pass (ask {pct(rub['by_kind']['ask'])}, "
                   f"what changed {pct(rub['by_kind']['what_changed'])}); means of 5: {means} |")

    def table(title: str, groups: dict[str, Any]) -> list[str]:
        lines = ["", f"## By {title}", "", "| | Cases | Refs valid | Grounded answers | Quantities grounded | "
                 "Experiments valid | Predictions held |", "| --- | --- | --- | --- | --- | --- | --- |"]
        for k, s in groups.items():
            lines.append(f"| {k} | {s['answered']}/{s['cases']} | {pct(s['refs_valid'])} | {pct(s['grounded_answers'])} | "
                         f"{pct(s['grounded_quantities'])} | {pct(s['try_valid'])} ({s['tries']}) | {pct(s['prediction_held'])} |")
        return lines

    out += table("kind", m["by_kind"]) + table("category", m["by_category"]) + table("level", m["by_level"])
    out += table("mode", m["by_mode"])

    done = [r for r in rows if r.get("checks")]
    bad_refs = [(r["id"], r["checks"]["invalid"]) for r in done if r["checks"]["invalid"]]
    if bad_refs:
        out += ["", "## Invalid references", ""] + [f"- `{i}`: {', '.join(v)}" for i, v in bad_refs]
    ung = [(r["id"], u) for r in done for u in r["checks"]["ungrounded"]]
    if ung:
        out += ["", f"## Ungrounded quantities ({len(ung)})", "", "| Case | Quantity | Why |", "| --- | --- | --- |"]
        out += [f"| `{i}` | {_cell(u['text'], 30)} | {_cell(u['why'])} |" for i, u in ung]
    tried = [r for r in done if r["checks"]["try"]]
    if tried:
        out += ["", "## Experiments", "", "| Case | Valid | Prediction | Check | Measured | Held |", "| --- | --- | --- | --- | --- | --- |"]
        for r in tried:
            c, p = r["checks"], r["checks"].get("prediction") or {}
            problems = "" if c["try_valid"] else " " + _cell("; ".join(str(x) for x in r["answer"]["try"].get("problems") or []), 60)
            out.append(f"| `{r['id']}` | {'yes' if c['try_valid'] else 'no' + problems} | {_cell(r['answer']['try'].get('predict') or '', 50)} | "
                       f"{p.get('check') or '–'} | {fmt(p['measured']) if p.get('measured') is not None else '–'} | "
                       f"{'–' if p.get('held') is None else ('yes' if p['held'] else 'no')} |")
    fails = [r for r in done if r.get("rubric") and r["rubric"].get("verdict") == "fail"]
    if fails:
        out += ["", "## Judged fail", ""] + [f"- `{r['id']}`: {_cell(r['rubric'].get('notes') or '', 300)}" for r in fails]
    failed = [r for r in rows if not r.get("checks")]
    if failed:
        out += ["", "## No answer", ""] + [f"- `{r['id']}`: {(r.get('error') or {}).get('code')} {(r.get('error') or {}).get('message') or ''}" for r in failed]
    calls = m.get("calls") or {}
    if calls:
        out += ["", "## Model calls", "", "| Kind | Calls | Errors | In tokens | Cached | Out tokens | p50 ms | p95 ms | Models |",
                "| --- | --- | --- | --- | --- | --- | --- | --- | --- |"]
        for kind, s in calls.items():
            out.append(f"| {kind} | {s['calls']} | {s['errors'] or ''} | {s['in_tokens']:,} | {s['cached_tokens']:,} | "
                       f"{s['out_tokens']:,} | {num(s['ms_p50'])} | {num(s['ms_p95'])} | {', '.join(s['models'])} |")
    out += ["", "## Cases", "", "| Case | Kind | Level | Mode | Refs | Grounded | Words | Try | Judge |",
            "| --- | --- | --- | --- | --- | --- | --- | --- | --- |"]
    for r in rows:
        c = r.get("checks")
        if not c:
            out.append(f"| `{r['id']}` | {r['kind']} | {r['level']} | {r.get('mode')} | no answer | | | | |")
            continue
        judge = (r.get("rubric") or {}).get("verdict") or ""
        out.append(f"| `{r['id']}` | {r['kind']} | {r['level']} | {r.get('mode')} | {c['refs_valid']}/{c['refs']} | "
                   f"{c['grounded']}/{c['quantities']} | {c['words']} | {'–' if not c['try'] else ('valid' if c['try_valid'] else 'invalid')} | {judge} |")
    return "\n".join(out) + "\n"


# ---------------------------------------------------------------- review sheet

REVIEW_COLUMNS = ["id", "kind", "level", "mode", "question", "answer", *(f"judge_{k}" for k in CRITERIA), "judge_verdict",
                  "judge_notes", *(f"human_{k}" for k in CRITERIA), "human_verdict", "human_notes"]


def review_rows(rows: list[dict[str, Any]]) -> list[dict[str, Any]]:
    """The rubric sample as a sheet for a human reviewer: the judge's scores, and blank columns for
    the reviewer's on the same scale."""
    out = []
    for r in rows:
        if "rubric" not in (r.get("tags") or []) or not r.get("checks"):
            continue
        j = r.get("rubric") or {}
        out.append({"id": r["id"], "kind": r["kind"], "level": r["level"], "mode": r.get("mode"),
                    "question": r.get("question") or "What changed?", "answer": r["text"],
                    **{f"judge_{k}": j.get(k, "") for k in CRITERIA}, "judge_verdict": j.get("verdict", ""),
                    "judge_notes": j.get("notes", ""), **{f"human_{k}": "" for k in CRITERIA},
                    "human_verdict": "", "human_notes": ""})
    return out


def agreement(sheet: Iterable[dict[str, str]]) -> dict[str, Any]:
    """How a reviewer's filled sheet agrees with the judge: the verdicts, and per criterion the mean
    absolute difference and the share within one point."""
    rows = [r for r in sheet if (r.get("human_verdict") or "").strip()]
    out: dict[str, Any] = {"reviewed": len(rows), "verdict_agreement": ratio(
        sum(1 for r in rows if r["human_verdict"].strip().lower() == (r.get("judge_verdict") or "").strip().lower()), len(rows))}
    for k in CRITERIA:
        pairs = [(float(r[f"judge_{k}"]), float(r[f"human_{k}"])) for r in rows
                 if str(r.get(f"judge_{k}", "")).strip() and str(r.get(f"human_{k}", "")).strip()]
        out[k] = {"n": len(pairs), "mean_abs_diff": mean([abs(a - b) for a, b in pairs]),
                  "within_one": ratio(sum(1 for a, b in pairs if abs(a - b) <= 1), len(pairs))}
    return out


# ---------------------------------------------------------------- commands

COMPARE = [
    ("refs_valid", "Refs valid", pct), ("grounded_answers", "Grounded answers", pct),
    ("grounded_quantities", "Quantities grounded", pct), ("arithmetic_errors", "Arithmetic errors", num),
    ("try_share", "Answers with an experiment", pct), ("try_valid", "Experiments valid", pct),
    ("try_when_expected", "Experiment when expected", pct), ("prediction_held", "Predictions held", pct),
    ("cite_met", "Expected citations", pct), ("socratic_asks_question", "Socratic asks a question", pct),
    ("words_mean", "Words (mean)", num), ("over_word_limit", f"Over {WORD_LIMIT} words", num),
    ("rubric_pass", "Rubric pass", pct), ("tokens_in_mean", "Input tokens", num),
    ("tokens_out_mean", "Output tokens", num), ("tokens_out_p95", "Output tokens p95", num),
    ("first_token_p50", "First token p50 (s)", lambda x: num(x, 1)), ("answer_p50", "Answer p50 (s)", lambda x: num(x, 1)),
    ("answer_p95", "Answer p95 (s)", lambda x: num(x, 1)),
]


def compare(reports: list[dict[str, Any]]) -> str:
    heads = [f"{r['meta']['tiers']} · {r['meta'].get('effort') or 'default'}" for r in reports]
    out = ["| | " + " | ".join(heads) + " |", "| --- |" + " --- |" * len(reports)]
    for key, label, f in COMPARE:
        timed = key.startswith(("first_token", "answer_"))  # a replay's times are not the model's
        out.append(f"| {label} | " + " | ".join(
            "–" if timed and not r["metrics"].get("latency_measured") else f(r["metrics"].get(key)) for r in reports) + " |")
    rubric_means = [r["metrics"].get("rubric") for r in reports]
    for k in CRITERIA:
        out.append(f"| Rubric {k} (of 5) | " + " | ".join(num(x["means"][k], 2) if x else "–" for x in rubric_means) + " |")
    return "\n".join(out) + "\n"


def main(argv: list[str]) -> int:
    if hasattr(sys.stdout, "reconfigure"):
        sys.stdout.reconfigure(encoding="utf-8", errors="replace")
    if len(argv) >= 2 and argv[0] == "compare":
        reports = [json.loads((Path(d) / "report.json").read_text(encoding="utf-8")) for d in argv[1:]]
        print(compare(reports), end="")
        return 0
    if len(argv) == 2 and argv[0] == "agreement":
        with open(argv[1], encoding="utf-8-sig", newline="") as f:
            print(json.dumps(agreement(csv.DictReader(f)), indent=1))
        return 0
    print(__doc__.split("\n\n")[1])
    return 2


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
