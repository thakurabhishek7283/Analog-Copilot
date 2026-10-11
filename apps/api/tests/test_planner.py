"""Plan validation on its own (no model, no databases): every problem a re-plan is asked to fix, and
the plan a valid one becomes (commit order, block ids, links as port bindings)."""

from __future__ import annotations

import json

import circuit_core as cc
import pytest

from tutor_api.config import REPO
from tutor_api.llm.prompts import Prompts
from tutor_api.orchestrator.planner import validate
from tutor_api.orchestrator.schemas import plan_model, provider_schema


@pytest.fixture(scope="module")
def reg() -> cc.Registry:
    return cc.load_registry_dir(REPO / "registry")


@pytest.fixture(scope="module")
def prompts(reg) -> Prompts:
    return Prompts(reg)


def raw(prompts, blocks, links=(), uncovered=(), **extras):
    model = plan_model(tuple(prompts.templates))
    return model.model_validate({
        "blocks": [{"title": "", "purpose": "", "targets": []} | b for b in blocks],
        "links": [{"source": s, "target": t} for s, t in links],
        "uncovered": list(uncovered),
        **extras,
    })


def targets(**kw):
    return [{"name": k, "value": v} for k, v in kw.items()]


def test_a_valid_plan_commits_sources_first_with_their_links_bound(reg, prompts):
    session = cc.Session(reg)
    p = raw(prompts, [
        {"id": "b1", "template": "sallen_key_lp", "title": "Filter", "targets": targets(fc_hz="2k")},
        {"id": "b2", "template": "sine_source", "title": "Signal", "targets": targets(freq_hz="500")},
    ], links=[("b2.out", "b1.in")])
    v = validate(p, session, prompts, 1)
    assert v.problems == [] and v.uncovered == []
    first, second = v.plan.blocks
    # The source commits first, and blocks are numbered in commit order.
    assert (first.plan_id, first.id, first.template) == ("b2", "b1", "sine_source")
    assert (second.plan_id, second.id, second.inputs) == ("b1", "b2", {"in": ("b1", "out")})
    assert second.ghost() == {
        "id": "b2", "title": "Sallen-Key low-pass (2nd order)", "role": "filter",
        "ports": [{"name": n, "direction": d} for n, d in [("in", "input"), ("out", "output"), ("vcc", "power_pos"),
                                                           ("vee", "power_neg"), ("gnd", "ground")]],
    }
    assert session.rev == 0  # validation built the plan in a fork


def test_block_ids_continue_after_the_circuits_own(reg, prompts):
    session = cc.Session(reg)
    ops = cc.unwrap(session.insert_block(json.dumps({"template": "rc_lowpass"})))["ops"]
    cc.unwrap(session.apply_ops(json.dumps(ops), "template"))
    v = validate(raw(prompts, [{"id": "b1", "template": "inverting_amp"}]), session, prompts, 1)
    assert v.plan.blocks[0].id == "b2" and v.plan.blocks[0].plan_id == "b1"


@pytest.mark.parametrize("source, new_id", [("b1.out", "b3"), ("existing.b1.out", "b1")])
def test_existing_outputs_bind_without_rebuilding_or_changing_them(reg, prompts, source, new_id):
    session = cc.Session(reg)
    ins = cc.unwrap(session.insert_block(json.dumps({"template": "sine_source"})))
    cc.unwrap(session.apply_ops(json.dumps(ins["ops"]), "template"))
    before = session.snapshot()
    v = validate(raw(prompts, [{"id": new_id, "template": "rc_lowpass"}],
                     links=[(source, f"{new_id}.in")]), session, prompts, 1)
    assert v.problems == []
    block = v.plan.blocks[0]
    assert block.id == "b2" and block.inputs == {"in": ("b1", "out")}
    # The job binds the actual committed id; it must have the pre-existing nets available too.
    from tutor_api.orchestrator.job import Job
    job = object.__new__(Job)
    job.nets = {"b1": {p["name"]: p["net"] for p in json.loads(before)["blocks"]["b1"]["ports"]}}
    assert job.bindings(block) == {"in": {"net": "B1_OUT"}}
    assert session.snapshot() == before


def test_existing_inputs_and_power_ports_cannot_be_used_as_outputs(reg, prompts):
    session = cc.Session(reg)
    ins = cc.unwrap(session.insert_block(json.dumps({"template": "rc_lowpass"})))
    cc.unwrap(session.apply_ops(json.dumps(ins["ops"]), "template"))
    blocks = [{"id": "b2", "template": "rc_highpass"}]
    for port in ("in", "gnd"):
        v = validate(raw(prompts, blocks, links=[(f"existing.b1.{port}", "b2.in")]), session, prompts, 1)
        assert v.plan is None and any("not an output" in p for p in v.problems)
    v = validate(raw(prompts, blocks, links=[("b2.out", "existing.b1.in")]), session, prompts, 1)
    assert v.plan is None and any("only new blocks can receive links" in p for p in v.problems)


def test_requested_measurements_resolve_plan_ids_in_commit_order(reg, prompts):
    p = raw(prompts, [{"id": "b1", "template": "rc_lowpass"}, {"id": "b2", "template": "sine_source"}],
            links=[("b2.out", "b1.in")],
            transient_tests={"source": "b2.out", "frequencies": ["100", "1k", "5k"], "outputs": ["b1.out"]},
            max_breadboards=2)
    v = validate(p, cc.Session(reg), prompts, 1)
    assert v.problems == []
    assert v.plan.transient_tests.source == ("b1", "out")
    assert v.plan.transient_tests.outputs == [("b2", "out")]
    assert v.plan.transient_tests.frequencies == [100, 1000, 5000]
    assert v.plan.max_breadboards == 2


@pytest.mark.parametrize("source, frequency, output", [("b9.out", "100", "b2.out"),
                                                     ("b2.out", "100", "b2.out"),
                                                     ("b1.out", "0", "b2.out"),
                                                     ("b1.out", "100", "b2.gnd")])
def test_invalid_measurement_requests_replan(reg, prompts, source, frequency, output):
    p = raw(prompts, [{"id": "b1", "template": "sine_source"}, {"id": "b2", "template": "rc_lowpass"}],
            transient_tests={"source": source, "frequencies": [frequency], "outputs": [output]})
    v = validate(p, cc.Session(reg), prompts, 1)
    assert v.plan is None and any("transient test:" in problem for problem in v.problems)


@pytest.mark.parametrize(
    "blocks, links, problem",
    [
        ([], [], "the plan has no blocks"),
        ([{"id": "x1", "template": "rc_lowpass"}], [], "block id 'x1' is not b1, b2, ..."),
        ([{"id": "b1", "template": "rc_lowpass"}, {"id": "b1", "template": "rc_highpass"}], [], "two blocks are called b1"),
        ([{"id": "b1", "template": "rc_lowpass", "targets": targets(fc_hz="1k") * 2}], [], "b1: target fc_hz is given twice"),
        ([{"id": "b1", "template": "rc_lowpass", "targets": targets(fc_hz="500k")}], [],
         "b1 (rc_lowpass): Cutoff frequency must be 10Hz to 100kHz"),
        ([{"id": "b1", "template": "rc_lowpass", "targets": targets(q="2")}], [], "b1 (rc_lowpass): rc_lowpass has no target q"),
        ([{"id": "b1", "template": "rc_lowpass"}], [("b1.out", "b2.in")], "link b1.out -> b2.in: there is no block b2"),
        ([{"id": "b1", "template": "rc_lowpass"}, {"id": "b2", "template": "rc_highpass"}], [("b1.in", "b2.in")],
         "link b1.in -> b2.in: b1.in is input, not an output"),
        ([{"id": "b1", "template": "rc_lowpass"}, {"id": "b2", "template": "rc_highpass"}], [("b1.out", "b2.gnd")],
         "link b1.out -> b2.gnd: b2.gnd is ground, not an input; power and ground join through the rails"),
        ([{"id": "b1", "template": "rc_lowpass"}, {"id": "b2", "template": "rc_highpass"}], [("b1.out", "b2.input")],
         "link b1.out -> b2.input: b2 (rc_highpass) has no port input; its ports are in out gnd"),
        ([{"id": "b1", "template": "rc_lowpass"}], [("b1", "b1.in")], "link b1 -> b1.in: write ports as block.port, e.g. b1.out"),
        ([{"id": "b1", "template": "rc_lowpass"}, {"id": "b2", "template": "rc_highpass"}, {"id": "b3", "template": "sine_source"}],
         [("b1.out", "b2.in"), ("b3.out", "b2.in")], "b2.in is linked twice"),
        ([{"id": "b1", "template": "rc_lowpass"}, {"id": "b2", "template": "rc_highpass"}],
         [("b1.out", "b2.in"), ("b2.out", "b1.in")], "the links make a loop through b1, b2"),
    ],
)
def test_plan_problems(reg, prompts, blocks, links, problem):
    v = validate(raw(prompts, blocks, links), cc.Session(reg), prompts, 1)
    assert v.plan is None and problem in v.problems, v.problems


def test_a_rail_the_circuit_holds_at_another_voltage_is_a_plan_problem(reg, prompts):
    """A divider on a 5 V VCC, then a Sallen-Key that needs VCC at 9..15 V."""
    session = cc.Session(reg)
    ops = cc.unwrap(session.insert_block(json.dumps({"template": "divider_bias", "ports": {"vcc": {"rail": {"net": "VCC", "volts": 5}}}})))["ops"]
    cc.unwrap(session.apply_ops(json.dumps(ops), "template"))
    v = validate(raw(prompts, [{"id": "b1", "template": "sallen_key_lp"}]), session, prompts, 1)
    assert v.plan is None and any(p.startswith("b1 (sallen_key_lp):") and "VCC" in p for p in v.problems), v.problems


def test_uncovered_needs_are_reported_separately(reg, prompts):
    v = validate(raw(prompts, [{"id": "b1", "template": "rc_lowpass"}], uncovered=["a microcontroller"]),
                 cc.Session(reg), prompts, 1)
    assert v.problems == [] and v.plan is not None
    assert v.uncovered == ["no template builds 'a microcontroller': leave it out of the request, or plan it from the templates"]


def test_a_block_without_targets_takes_every_default(reg, prompts):
    """GPT-6 Luna leaves `targets` out on templates that have none (12 re-plans in its live run)."""
    model = plan_model(tuple(prompts.templates))
    p = model.model_validate({"blocks": [{"id": "b1", "template": "rc_lowpass", "title": "", "purpose": ""}],
                              "links": [], "uncovered": []})
    v = validate(p, cc.Session(reg), prompts, 1)
    assert v.problems == [] and v.plan.blocks[0].targets == {}
    assert "targets" in provider_schema(model)["properties"]["blocks"]["items"]["required"]  # still asked for
