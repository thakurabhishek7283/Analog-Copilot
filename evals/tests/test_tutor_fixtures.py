"""The tutor eval cases and their frozen circuits: well formed, still what circuits.yaml and the cases'
edits build on the current registry (else run evals/tutor/fixtures.py), and every case's context
builds inside the tutor's budget."""

from __future__ import annotations

import json
from collections import Counter

import circuit_core as cc
import pytest

import fixtures as fx
from run_tutor_evals import batches, load_cases, request

ASK = {"why_value", "role", "change_spec", "predict", "read_sim", "debug", "missing_data", "wrong_premise", "off_topic",
       "follow_up"}
WHAT_CHANGED = {"check_fails", "check_recovers", "value_tweak", "swap", "add_remove", "rewire", "nothing_electrical",
                "analysis", "multi_edit"}


@pytest.fixture(scope="module")
def reg() -> cc.Registry:
    return fx.registry()


@pytest.fixture(scope="module")
def fixtures() -> dict:
    return json.loads(fx.FIXTURES.read_text(encoding="utf-8"))


CASES = load_cases(fx.CASES)


def test_cases_are_well_formed(fixtures):
    ids = [c["id"] for c in CASES]
    assert len(ids) == len(set(ids)) == 130
    kinds = Counter(c["kind"] for c in CASES)
    assert kinds == {"ask": 100, "what_changed": 30}
    assert Counter(c["kind"] for c in CASES if "rubric" in (c.get("tags") or [])) == {"ask": 45, "what_changed": 15}
    by_id = {c["id"]: c for c in CASES}
    for c in CASES:
        assert c["circuit"] in fixtures["circuits"], c["id"]
        assert c["level"] in ("beginner", "intermediate", "advanced") and c["mode"] in ("explain", "socratic"), c["id"]
        assert c["category"] in (ASK if c["kind"] == "ask" else WHAT_CHANGED), c["id"]
        assert set(c.get("expect") or {}) <= {"cite", "try", "declines"}, c["id"]
        assert (c.get("expect") or {}).get("try") in (None, "yes", "no"), c["id"]
        if c["kind"] == "ask":
            assert c["question"].strip() and "edit" not in c, c["id"]
        else:
            assert c["edit"] and "question" not in c and "selection" not in c, c["id"]
        # A follow-up has earlier turns, or comes after a "What changed?" on the same circuit; only follow-ups do.
        assert (c["category"] == "follow_up") == bool(c.get("turns") or c.get("after")), c["id"]
        assert all(isinstance(q, str) and q.strip() for q in c.get("turns") or []), c["id"]
        if c.get("after"):
            first = by_id[c["after"]]
            assert first["kind"] == "what_changed" and first["circuit"] == c["circuit"], c["id"]
    assert sum(1 for c in CASES if c["category"] == "follow_up") == 10
    socratic = sum(1 for c in CASES if c["mode"] == "socratic")
    assert socratic >= 20, "about a fifth of the cases in Socratic mode"


def test_fixtures_are_what_the_yaml_builds(reg, fixtures):
    built = fx.structure(fx.load_yaml(fx.CIRCUITS), fx.load_yaml(fx.CASES), reg)["fixtures"]
    assert fixtures["registry_version"] == reg.version, "run evals/tutor/fixtures.py"
    for name, c in built["circuits"].items():
        frozen = fixtures["circuits"].get(name)
        assert frozen is not None and (frozen["batches"], frozen["rev"]) == (c["batches"], c["rev"]), (
            f"circuit {name} changed: run evals/tutor/fixtures.py")
    assert set(fixtures["circuits"]) == set(built["circuits"])
    for name, e in built["edits"].items():
        frozen = fixtures["edits"].get(name)
        assert frozen is not None and (frozen["ops"], frozen["rev"]) == (e["ops"], e["rev"]), (
            f"the edit of {name} changed: run evals/tutor/fixtures.py")
    assert set(fixtures["edits"]) == set(built["edits"])


def test_every_simulated_circuit_has_its_values(fixtures):
    unsimulated = {c["id"] for c in fx.load_yaml(fx.CIRCUITS) if c.get("sim") == "none"}
    for name, c in fixtures["circuits"].items():
        assert (c["sim"] == {}) == (name in unsimulated), name
        if name not in unsimulated:
            assert c["sim"]["status"] in ("ok", "singular_matrix"), name
    assert all(e["sim"]["status"] == "ok" for e in fixtures["edits"].values())
    assert fixtures["circuits"]["sk_lp"]["sim"]["ac"]["hz"] == pytest.approx(1000, rel=0.02), "the source's frequency"


@pytest.mark.parametrize("case", CASES, ids=[c["id"] for c in CASES])
def test_every_case_has_a_context_inside_the_budget(case, reg, fixtures):
    _, body = request(case, fixtures, None)
    before = fx.session_at(reg, fixtures["circuits"][case["circuit"]]["batches"])
    now = fx.session_at(reg, batches(case, fixtures))  # after the case's edit, or the edit a follow-up comes after
    assert now.rev == body["rev"]
    if case["kind"] == "what_changed":
        ctx = cc.unwrap(now.tutor_changes(before, json.dumps(body)))
    else:
        ctx = cc.unwrap(now.tutor_context(json.dumps(body)))
    assert ctx["tokens"] <= 2000
    for ref in (case.get("expect") or {}).get("cite") or []:
        kind, _, rid = ref.rpartition(":")
        listed = {"": ctx["parts"], "net": ctx["nets"], "block": ctx["blocks"]}[kind]
        assert rid in listed, f"{case['id']} expects a citation of {ref}, which its context does not list"
