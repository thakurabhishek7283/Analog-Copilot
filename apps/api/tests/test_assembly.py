"""Assembly verification and bounded repair on the real core and native ngspice.

Only the queue/database transport is replaced: rejected trials must never reach commit.
"""

import asyncio
import copy
import json
from pathlib import Path
from types import SimpleNamespace

import circuit_core as cc
import pytest
from sim_runner import ngspice_batch as nb
from sim_runner.protocol import SimSummary

from tutor_api.jobs.runner import Failure
from tutor_api.llm.fake import FakeProvider
from tutor_api.llm.gateway import Gateway
from tutor_api.llm.prompts import Prompts
from tutor_api.models.contract import JobEvent
from tutor_api.orchestrator import assembly
from tutor_api.orchestrator import intent

ROOT = Path(__file__).resolve().parents[3]


@pytest.fixture
def registry():
    return cc.load_registry_dir(ROOT / "registry")


def insert(session, template, *, ports=None, targets=None, id=None):
    request = {"template": template, "ports": ports or {}, "targets": targets or {}}
    if id:
        request["id"] = id
    block = cc.unwrap(session.insert_block(json.dumps(request)))
    cc.unwrap(session.apply_ops(json.dumps(block["ops"]), "template"))
    return block


def circuit(registry, *, source=True, chain=False):
    session = cc.Session(registry)
    if source:
        insert(session, "sine_source", targets={"freq_hz": "100"})
    insert(session, "rc_lowpass", ports={"in": {"net": "B1_OUT"}} if source else {})
    if chain:
        insert(session, "rc_lowpass", ports={"in": {"net": "B2_OUT"}})
    return session


@pytest.fixture
def native(monkeypatch):
    if nb.ngspice_path() is None:
        if __import__("os").environ.get("REQUIRE_NGSPICE"):
            pytest.fail("native ngspice is required")
        pytest.skip("native ngspice not built")
    calls = []

    async def simulate(redis, req, **kwargs):
        calls.append(req)
        r = await asyncio.to_thread(nb.simulate, req.netlist, req.includes, hash=req.hash, vectors=False)
        return SimSummary(r.hash, r.status, r.meas, r.failed_meas, r.log, r.ms)

    monkeypatch.setattr(assembly.sim_client, "simulate", simulate)
    return calls


class Context:
    def __init__(self, session, registry):
        self.current = session
        self.registry_version = registry.version
        self.job_id = "assembly-test"
        self.redis = None
        self.events = []
        self.commits = []
        self.plans = []
        self.lessons = []

    async def session(self):
        return self.current.fork()

    async def set_state(self, state):
        await self.emit("job.state", {"state": state})

    async def emit(self, event, data):
        JobEvent.model_validate({"event": event, "data": data})
        self.events.append((event, copy.deepcopy(data)))

    async def set_plan(self, plan):
        self.plans.append(copy.deepcopy(plan))

    async def commit(self, ops, *, author, repair_attempt, base_rev):
        assert self.current.rev == base_rev
        envs = []
        for op in ops:
            env = {"v": 1, "seq": self.current.rev + 1, "base_rev": self.current.rev,
                   "author": author, "job": self.job_id, **op}
            cc.unwrap(self.current.apply(json.dumps(env)))
            envs.append(env)
        self.commits.append(ops)
        await self.emit("circuit.patch", {"ops": envs, "attempt": repair_attempt})


def setup(session, registry, replies):
    ctx = Context(session, registry)
    provider = FakeProvider({"assembly_repair": replies, "rules": [{
        "kind": "assembly_audit", "when": [], "reply": {
            "requirements": [{"text": "Two cascaded low-pass stages", "status": "met",
                              "evidence": "Both stages and their checked outputs are present.",
                              "blocks": ["b2"], "checks": ["b2.fc_hz"]}]}}]})

    async def lesson(kind, text):
        ctx.lessons.append((kind, text))

    job = SimpleNamespace(ctx=ctx, gw=Gateway(provider, retries=0), prompts=Prompts(registry),
                          prompt="Two cascaded 1 kHz RC low-pass stages", templates_only=False,
                          usage=lambda response: None, lesson=lesson)
    plan = SimpleNamespace(to_json=lambda: {"blocks": [], "rounds": 1})
    return job, plan, ctx, provider


def repair(*changes):
    return {"explanation": "Increase the second stage's impedance to reduce loading.",
            "unsupported": "", "changes": list(changes)}


GOOD_REPAIR = repair(
    {"kind": "set_param", "refdes": "R2", "key": "resistance", "value": "1.6meg"},
    {"kind": "set_param", "refdes": "C2", "key": "capacitance", "value": "100p"},
)


async def test_complete_circuit_passes_and_missing_stimulus_is_incomplete(registry, native):
    session = circuit(registry)
    verdict = await assembly.verify(session, None, registry.version)
    assert verdict.status == "passed", verdict
    assert {f"{c['block']}.{c['name']}" for c in verdict.checks} == {
        "b1.amplitude_v", "b1.freq_hz", "b2.fc_hz"
    }
    assert all(c["pass"] for c in verdict.checks)
    assert abs(next(c["measured"] for c in verdict.checks if c["name"] == "freq_hz") - 100) < 2
    count = len(native)
    verdict = await assembly.verify(circuit(registry, source=False), None, registry.version)
    assert verdict.status == "incomplete" and len(native) == count


async def test_isolated_blocks_pass_but_loading_fails_then_one_atomic_repair_passes(registry, native):
    isolated = circuit(registry)
    assert (await assembly.verify(isolated, None, registry.version)).status == "passed"
    session = circuit(registry, chain=True)
    original = await assembly.verify(session, None, registry.version)
    assert original.status == "failed"
    checks = {c["block"]: c for c in original.checks}
    assert not checks["b2"]["pass"]
    # The downstream stage is measured relative to its input, not the original source.
    assert checks["b3"]["pass"], checks
    before = json.loads(session.snapshot())
    job, plan, ctx, provider = setup(session, registry, [GOOD_REPAIR])
    await assembly.run(job, plan)
    assert len([c for c in provider.calls if c.kind == "assembly_repair"]) == len(ctx.commits) == 1
    final = ctx.plans[-1]["verification"]
    assert final["status"] == "passed" and final["attempt"] == 1
    assert final["rev"] == session.rev
    after = json.loads(session.snapshot())
    assert before["blocks"] == after["blocks"]
    assert len(before["parts"]) == len(after["parts"])
    assert [e for e, _ in ctx.events].count("circuit.patch") == 1
    assert ctx.events[-1][0] == "circuit.summary"


async def test_two_failed_attempts_preserve_the_committed_circuit(registry, native):
    session = circuit(registry, chain=True)
    before = session.snapshot()
    bad = repair({"kind": "set_param", "refdes": "R2", "key": "resistance", "value": "100"})
    job, plan, ctx, provider = setup(session, registry, [bad, bad, GOOD_REPAIR])
    with pytest.raises(Failure) as failure:
        await assembly.run(job, plan)
    assert failure.value.code == "assembly_verification_failed" and not failure.value.retryable
    assert len([c for c in provider.calls if c.kind == "assembly_repair"]) == 2 and ctx.commits == []
    assert session.snapshot() == before
    assert ctx.plans[-1]["verification"]["status"] == "failed"
    assert len(ctx.plans[-1]["assembly_attempts"]) == 2


async def test_invalid_repair_feedback_reaches_the_second_attempt(registry, native):
    session = circuit(registry, chain=True)
    bad = repair({"kind": "set_param", "refdes": "R999", "key": "resistance", "value": "100"})
    job, plan, ctx, provider = setup(session, registry, [bad, GOOD_REPAIR])
    await assembly.run(job, plan)
    assert "R999" in [c for c in provider.calls if c.kind == "assembly_repair"][1].user
    assert ctx.plans[-1]["verification"]["attempt"] == 2
    assert len(ctx.commits) == 1


def test_a_repair_cannot_remove_or_weaken_a_specification(registry):
    session = circuit(registry, chain=True)
    before = session.snapshot()
    removal = {"kind": "remove_block", "id": "b3"}
    with pytest.raises(ValueError, match="preserve b3"):
        assembly.trial(session, assembly.Repair.model_validate(repair(removal)), "test")
    replacement = {"kind": "insert_block", "id": "b3", "template": "rc_lowpass",
                   "targets": [{"name": "fc_hz", "value": "10k"}],
                   "ports": [{"name": "in", "value": "B2_OUT"}]}
    with pytest.raises(ValueError, match="preserve b3"):
        assembly.trial(session, assembly.Repair.model_validate(repair(removal, replacement)), "test")
    assert session.snapshot() == before


def test_a_repair_cannot_change_the_stimulus_to_hide_a_failure(registry):
    session = circuit(registry, chain=True)
    change = {"kind": "set_param", "refdes": "V1", "key": "frequency", "value": "10"}
    with pytest.raises(ValueError, match="preserve source V1"):
        assembly.trial(session, assembly.Repair.model_validate(repair(change)), "test")


async def test_template_mode_repairs_use_the_small_model(registry, native):
    job, plan, ctx, provider = setup(circuit(registry, chain=True), registry, [GOOD_REPAIR])
    job.templates_only = True
    await assembly.run(job, plan)
    assert [c for c in provider.calls if c.kind == "assembly_repair"][0].tier == "small"
    assert ctx.plans[-1]["verification"]["status"] == "passed"


async def test_audit_finds_a_request_omitted_by_the_plan(registry, native):
    session = circuit(registry)
    assert (await assembly.verify(session, None, registry.version)).status == "passed"
    missing = {"requirements": [
        {"text": "A 1 kHz low-pass filter", "status": "met", "evidence": "The filter passed its corner check.",
         "blocks": ["b2"], "checks": ["b2.fc_hz"]},
        {"text": "A second low-pass stage", "status": "missing", "evidence": "No second filter exists.",
         "blocks": [], "checks": []},
    ]}
    job, plan, ctx, _ = setup(session, registry, [])
    job.prompt = "A sine source feeding two cascaded 1 kHz low-pass stages"
    job.gw = Gateway(FakeProvider({"assembly_audit": [missing]}), retries=0)
    verdict = await assembly.assess(job, plan, session)
    assert verdict.status == "failed"
    assert len(verdict.requirements) == 2
    assert "second low-pass stage" in verdict.problems[0]
    assert ctx.commits == []


async def test_audit_refuses_claims_without_real_circuit_evidence(registry, native):
    session = circuit(registry)
    job, plan, _, _ = setup(session, registry, [])
    unsupported = {"requirements": [{"text": "A filter with 40 dB stopband rejection",
                                     "status": "met", "evidence": "Claimed by the plan.",
                                     "blocks": ["b2"], "checks": ["b2.rejection"]}]}
    job.gw = Gateway(FakeProvider({"assembly_audit": [unsupported]}), retries=0)
    verdict = await assembly.assess(job, plan, session)
    assert verdict.status == "incomplete"
    assert verdict.requirements[0]["status"] == "unverifiable"


async def test_missing_prompt_stage_is_added_and_rechecked_before_commit(registry, native):
    session = circuit(registry)
    before = session.snapshot()
    missing = {"requirements": [
        {"text": "Two cascaded 1 kHz filters", "status": "missing",
         "evidence": "The circuit has only one filter.", "blocks": [], "checks": []}]}
    met = {"requirements": [
        {"text": "Two cascaded 1 kHz filters", "status": "met",
         "evidence": "Both stages have passed corner checks and the second takes B2_OUT as input.",
         "blocks": ["b2", "b3"], "checks": ["b2.fc_hz", "b3.fc_hz"]}]}
    add_stage = repair(
        {"kind": "insert_block", "id": "b3", "template": "rc_lowpass",
         "targets": [{"name": "fc_hz", "value": "1k"}],
         "ports": [{"name": "in", "value": "B2_OUT"}]},
        {"kind": "set_param", "refdes": "R2", "key": "resistance", "value": "1.6meg"},
        {"kind": "set_param", "refdes": "C2", "key": "capacitance", "value": "100p"},
    )
    job, plan, ctx, _ = setup(session, registry, [add_stage])
    job.prompt = "A sine source feeding two cascaded 1 kHz low-pass filters"
    job.gw = Gateway(FakeProvider({"assembly_audit": [missing, met],
                                   "assembly_repair": [add_stage]}), retries=0)
    await assembly.run(job, plan)
    assert ctx.plans[-1]["verification"]["status"] == "passed"
    assert ctx.plans[-1]["verification"]["requirements"][0]["status"] == "met"
    assert len(ctx.commits) == 1
    assert "b3" in json.loads(session.snapshot())["blocks"]
    assert before != session.snapshot()


async def test_cancelling_during_repair_keeps_the_assembled_circuit(registry, native):
    session = circuit(registry, chain=True)
    before = session.snapshot()
    job, plan, ctx, _ = setup(session, registry, [])
    entered = asyncio.Event()

    async def waiting(req):
        entered.set()
        await asyncio.Event().wait()

    job.gw.complete = waiting
    task = asyncio.create_task(assembly.run(job, plan))
    await asyncio.wait_for(entered.wait(), 10)
    task.cancel()
    with pytest.raises(asyncio.CancelledError):
        await task
    assert ctx.commits == [] and session.snapshot() == before


async def test_simulator_timeouts_retry_without_spending_model_attempts(registry, monkeypatch):
    calls = []

    async def unavailable(*args, **kwargs):
        calls.append(1)
        raise TimeoutError

    monkeypatch.setattr(assembly.sim_client, "simulate", unavailable)
    job, plan, ctx, provider = setup(circuit(registry), registry, [GOOD_REPAIR])
    with pytest.raises(Failure, match="simulator timed out"):
        await assembly.run(job, plan)
    assert len(calls) == 3 and provider.calls == [] and ctx.commits == []
    assert ctx.plans[-1]["verification"]["status"] == "simulation_error"
