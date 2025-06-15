"""The generated Pydantic models must accept everything circuit-core emits and every envelope
it accepts. Run after tools/parity/run.sh (uses its logs): pytest tools/codegen"""

import json
import sys
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "apps/api"))

import circuit_core as cc  # noqa: E402
from tutor_api.models import contract as m  # noqa: E402

LOGS = ROOT / "target/parity/logs.json"


@pytest.fixture(scope="module")
def reg():
    return cc.load_registry_dir(ROOT / "registry")


@pytest.mark.skipif(not LOGS.exists(), reason="run tools/parity/run.sh first")
def test_models_accept_core_output(reg):
    logs = json.loads(LOGS.read_text(encoding="utf-8"))["logs"][:300]
    accepted = 0
    for log in logs:
        s = cc.Session(reg)
        for env in log:
            out = s.apply(env)
            m.Outcome.model_validate_json(out)
            if out.startswith('{"ok"'):
                m.OpEnvelope.model_validate_json(env)
                accepted += 1
        m.Circuit.model_validate_json(s.snapshot())
        for issue in json.loads(s.erc("user_edit"))["ok"]:
            m.ErcIssue.model_validate(issue)
        compiled = json.loads(s.compile())
        if "ok" in compiled:
            m.Netlist.model_validate(compiled["ok"])
        else:
            m.CompileError.model_validate(compiled["err"])
        good = [e for e in log if e.endswith("}")]
        m.Trial.model_validate(json.loads(cc.Session(reg).apply_all("[" + ",".join(good) + "]"))["ok"])
    assert accepted > 1000


def test_models_reject_what_core_rejects():
    with pytest.raises(Exception):
        m.OpEnvelope.model_validate({"v": 1, "seq": 1, "op": "part.explode", "author": "user", "base_rev": 0, "body": {}})
    with pytest.raises(Exception):
        m.OpEnvelope.model_validate(
            {"v": 1, "seq": 1, "op": "part.remove", "author": "user", "base_rev": 0, "body": {"refdes": "R1", "x": 1}}
        )


def test_registry_bundle_validates(reg):
    m.Registry.model_validate_json(reg.to_json())
