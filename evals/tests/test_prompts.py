"""evals/prompts.yaml is well formed and covers the template library."""

from __future__ import annotations

import re
from collections import Counter
from pathlib import Path

import circuit_core as cc
import pytest

from run_evals import HERE, LEVELS, REPO, load_cases

CASES = load_cases(HERE / "prompts.yaml")


@pytest.fixture(scope="module")
def templates() -> dict:
    import json

    return json.loads(cc.load_registry_dir(REPO / "registry").to_json())["templates"]


def test_there_are_100_prompts_with_unique_ids():
    # LLD 16: the Phase 2 eval harness runs 100 prompts.
    assert len(CASES) == 100
    ids = [c["id"] for c in CASES]
    assert len(set(ids)) == len(ids), [i for i, n in Counter(ids).items() if n > 1]
    assert all(re.fullmatch(r"[a-z0-9]+(-[a-z0-9]+)*", i) for i in ids)


def test_every_prompt_is_complete(templates):
    for c in CASES:
        assert set(c) <= {"id", "prompt", "level", "expect", "tags"}, c["id"]
        assert c["prompt"].strip() and c["level"] in LEVELS and c["tags"], c["id"]
        if c["expect"] == "unsupported":
            continue
        assert c["expect"], c["id"]
        for item in c["expect"]:
            for t in [item] if isinstance(item, str) else item:
                assert t in templates, f"{c['id']}: no template {t}"


def test_every_template_is_asked_for_at_every_level(templates):
    levels: dict[str, set[str]] = {t: set() for t in templates}
    for c in CASES:
        if c["expect"] == "unsupported":
            continue
        for item in c["expect"]:
            for t in [item] if isinstance(item, str) else item:
                levels[t].add(c["level"])
    assert {t: lv for t, lv in levels.items() if len(lv) < 2} == {}
    assert Counter(c["level"] for c in CASES).keys() == set(LEVELS)


def test_the_mix_has_chains_and_out_of_scope_requests():
    tags = Counter(t for c in CASES for t in c["tags"])
    assert tags["chain"] >= 20 and tags["single"] >= 50
    assert tags["scope"] == sum(1 for c in CASES if c["expect"] == "unsupported") >= 5
    assert tags["injection"] >= 2


def test_selection(tmp_path: Path):
    assert [c["id"] for c in load_cases(HERE / "prompts.yaml", only=["inv-10", "rc-lp-1k"])] == ["rc-lp-1k", "inv-10"]
    assert all("scope" in c["tags"] for c in load_cases(HERE / "prompts.yaml", tags=["scope"]))
    assert len(load_cases(HERE / "prompts.yaml", limit=3)) == 3
    with pytest.raises(SystemExit, match="no prompt nope"):
        load_cases(HERE / "prompts.yaml", only=["nope"])
