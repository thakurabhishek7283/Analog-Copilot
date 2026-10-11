"""The planner (LLD §6): the request becomes a `BlockPlan`, a list of blocks each built on a template,
plus links from output ports to input ports. A plan that fails validation goes back to the model
with its problems, at most twice.

Validation builds the whole plan from its templates in a fork of the circuit (the fallback path,
in the order blocks will commit): every target is in range, every rail fits, every link joins
an output to an input of blocks that exist, nothing is linked into twice, and there is no loop.
So the fallback of any block is known to apply, and the block ids are the ones the commits get.
"""

from __future__ import annotations

import asyncio
import json
import re
from collections.abc import Callable
from dataclasses import asdict, dataclass, field
from typing import Any

import circuit_core as cc

from ..llm.base import LlmRequest, LlmResponse, Tier
from ..llm.gateway import Gateway
from ..llm.prompts import Prompts
from .schemas import SchemaError, parse, plan_model, provider_schema

MAX_REPLANS = 2
PLAN_MAX_TOKENS = 3000
PORT = re.compile(r"^((?:existing\.)?b\d+)\.(\w+)$")
SIGNAL_OUT = {"output", "bidir"}
SIGNAL_IN = {"input", "bidir"}


@dataclass
class PlannedBlock:
    id: str  # the block's id in the circuit (the next free bN when it commits)
    plan_id: str  # the id the model gave it
    template: str
    role: str
    title: str  # the plan's name for it, in prompts and narration
    purpose: str
    targets: dict[str, str]
    ports: dict[str, str]  # template port -> direction
    block_title: str  # the committed block's title: its template's (the ghost shows the same)
    inputs: dict[str, tuple[str, str]] = field(default_factory=dict)  # input port -> (block id, output port)
    outcome: dict[str, Any] = field(default_factory=dict)

    def ghost(self) -> dict[str, Any]:
        return {
            "id": self.id, "title": self.block_title, "role": self.role,
            "ports": [{"name": n, "direction": d} for n, d in self.ports.items()],
        }

    def brief(self) -> dict[str, str]:
        return {"id": self.id, "template": self.template, "title": self.title, "purpose": self.purpose}


@dataclass
class TransientTests:
    source: tuple[str, str]
    frequencies: list[float]
    outputs: list[tuple[str, str]]


@dataclass
class Plan:
    blocks: list[PlannedBlock]  # in the order they commit: sources before what they drive
    rounds: int  # model calls it took
    transient_tests: TransientTests | None = None
    max_breadboards: int | None = None
    evidence: dict[str, Any] = field(default_factory=dict)

    def to_json(self) -> dict[str, Any]:
        out = {"rounds": self.rounds, "blocks": [asdict(b) for b in self.blocks]}
        if self.transient_tests:
            out["transient_tests"] = asdict(self.transient_tests)
        if self.max_breadboards:
            out["max_breadboards"] = self.max_breadboards
        if self.evidence:
            out["evidence"] = self.evidence
        return out


class PlanFailed(Exception):
    def __init__(self, problems: list[str], unsupported: bool):
        super().__init__("; ".join(problems))
        self.problems = problems
        self.unsupported = unsupported  # the last plan still lists `uncovered` items: the request is out of scope


@dataclass
class Validated:
    plan: Plan | None
    problems: list[str]
    uncovered: list[str]


def validate(raw: Any, session: cc.Session, prompts: Prompts, rounds: int) -> Validated:
    templates = prompts.templates
    problems: list[str] = []
    uncovered = [f"no template builds {u!r}: leave it out of the request, or plan it from the templates"
                 for u in raw.uncovered]
    if not raw.blocks and not raw.uncovered:
        problems.append("the plan has no blocks")
    by_id: dict[str, Any] = {}
    for b in raw.blocks:
        if not re.fullmatch(r"b\d+", b.id):
            problems.append(f"block id {b.id!r} is not b1, b2, ...")
        elif b.id in by_id:
            problems.append(f"two blocks are called {b.id}")
        by_id[b.id] = b
        names = [t.name for t in b.targets]
        for name in sorted({n for n in names if names.count(n) > 1}):
            problems.append(f"{b.id}: target {name} is given twice")

    existing = json.loads(session.snapshot())["blocks"]
    # An explicit prefix disambiguates existing circuit ids from the model's local plan ids.
    sources = {f"existing.{bid}": block for bid, block in existing.items()}
    sources.update({bid: block for bid, block in existing.items() if bid not in by_id})

    inputs: dict[str, dict[str, tuple[str, str]]] = {i: {} for i in by_id}
    for link in raw.links:
        ends = [PORT.match(link.source), PORT.match(link.target)]
        if not all(ends):
            problems.append(f"link {link.source} -> {link.target}: write ports as block.port, e.g. b1.out")
            continue
        (sb, sp), (tb, tp) = (m.groups() for m in ends)
        bad = False
        for blk, port, want, what in ((sb, sp, SIGNAL_OUT, "an output"), (tb, tp, SIGNAL_IN, "an input")):
            if blk not in by_id and blk not in sources:
                problems.append(f"link {link.source} -> {link.target}: there is no block {blk}")
                bad = True
                continue
            if blk in by_id:
                template = by_id[blk].template
                ports = templates[template]["ports"]
            else:
                template = sources[blk].get("template") or "existing block"
                ports = {p["name"]: p["direction"] for p in sources[blk]["ports"]}
            if port not in ports:
                problems.append(f"link {link.source} -> {link.target}: {blk} ({template}) has no port "
                                f"{port}; its ports are {' '.join(ports)}")
                bad = True
            elif ports[port] not in want:
                problems.append(f"link {link.source} -> {link.target}: {blk}.{port} is {ports[port]}, not {what}"
                                + ("; power and ground join through the rails" if ports[port] not in SIGNAL_IN | SIGNAL_OUT else ""))
                bad = True
        if bad:
            continue
        if tb not in by_id:
            problems.append(f"link {link.source} -> {link.target}: only new blocks can receive links; existing blocks are unchanged")
        elif sb == tb:
            problems.append(f"link {link.source} -> {link.target} joins a block to itself")
        elif tp in inputs[tb]:
            problems.append(f"{tb}.{tp} is linked twice")
        else:
            inputs[tb][tp] = (sb, sp)

    # Sources before what they drive; plan order otherwise.
    order: list[str] = []
    waiting = list(by_id)
    while waiting:
        ready = next((i for i in waiting if all(src in order or src in sources for src, _ in inputs[i].values())), None)
        if ready is None:
            problems.append(f"the links make a loop through {', '.join(waiting)}")
            break
        order.append(ready)
        waiting.remove(ready)
    if problems:
        return Validated(None, problems, uncovered)

    # Build the plan from its templates in a fork, as the fallbacks would.
    fork = session.fork()
    nets = {key: {p["name"]: p["net"] for p in block["ports"]} for key, block in sources.items()}
    ids = {key: block["id"] for key, block in sources.items()}
    for pid in order:
        b = by_id[pid]
        ports = {p: {"net": nets[src][sp]} for p, (src, sp) in inputs[pid].items() if src in nets}
        req = {"template": {"template": b.template, "targets": {t.name: t.value for t in b.targets}, "ports": ports}}
        trial = cc.unwrap(fork.trial_block(json.dumps(req)))
        if trial["problems"]:
            problems += [f"{pid} ({b.template}): {p['message']}" for p in trial["problems"]]
            continue
        cc.unwrap(fork.apply_ops(json.dumps(trial["ops"]), "template"))
        begin = trial["ops"][0]["body"]
        nets[pid] = {p["name"]: p["net"] for p in begin["ports"]}
        ids[pid] = trial["block"]
    if problems:
        return Validated(None, problems, uncovered)

    tests = None
    if raw.transient_tests:
        test = raw.transient_tests
        resolved = []
        for endpoint in [test.source, *test.outputs]:
            match = PORT.fullmatch(endpoint)
            if not match or match[1] not in nets or match[2] not in nets[match[1]]:
                problems.append(f"transient test: unknown port {endpoint}")
                continue
            bid, port = match.groups()
            tpl = by_id[bid].template if bid in by_id else sources[bid].get("template")
            ports = templates[tpl]["ports"] if tpl in templates else {}
            if ports.get(port) not in SIGNAL_OUT:
                problems.append(f"transient test: {endpoint} must be an output")
            if endpoint == test.source and tpl != "sine_source":
                problems.append("transient test: source must be a sine_source block")
            resolved.append((ids[bid], port))
        frequencies = []
        for value in test.frequencies:
            quantity = json.loads(cc.parse_quantity(value, "hertz"))
            hz = quantity.get("ok", {}).get("si", 0)
            if not 1 <= hz <= 100_000:
                problems.append(f"transient test: frequency {value!r} must be 1 Hz to 100 kHz")
            frequencies.append(hz)
        if problems:
            return Validated(None, problems, uncovered)
        tests = TransientTests(resolved[0], list(dict.fromkeys(frequencies)), resolved[1:])

    blocks = []
    for pid in order:
        b = by_id[pid]
        t = templates[b.template]
        blocks.append(PlannedBlock(
            id=ids[pid], plan_id=pid, template=b.template, role=t["role"], title=b.title.strip() or t["title"],
            purpose=b.purpose.strip(), targets={x.name: x.value for x in b.targets}, ports=dict(t["ports"]),
            block_title=t["title"],
            inputs={p: (ids[src], sp) for p, (src, sp) in inputs[pid].items()},
        ))
    return Validated(Plan(blocks, rounds, tests, raw.max_breadboards), [], uncovered)


async def plan(gw: Gateway, prompts: Prompts, session: cc.Session, *, prompt: str, level: str, tier: Tier,
               on_usage: Callable[[LlmResponse], None]) -> Plan:
    """A valid plan, or `PlanFailed` after the re-plans; `LlmUnavailable` when no provider answers."""
    model = plan_model(tuple(prompts.templates))
    schema = provider_schema(model)
    circuit = session.circuit_text()
    previous: str | None = None
    problems: list[str] = []
    unsupported = False
    for round_ in range(1, MAX_REPLANS + 2):
        req = LlmRequest(
            "plan", tier, prompts.system,
            prompts.plan(prompt=prompt, level=level, circuit=circuit, previous=previous, problems=problems),
            schema=schema, schema_name="plan", max_tokens=PLAN_MAX_TOKENS,
        )
        resp = await gw.complete(req)
        on_usage(resp)
        try:
            raw = parse(model, resp.text, resp.finish_reason)
        except SchemaError as e:
            previous, problems, unsupported = resp.text[:4000], [f"schema_error: {e}"], False
            continue
        v = await asyncio.to_thread(validate, raw, session, prompts, round_)
        if v.plan is not None and not v.uncovered:
            return v.plan
        previous = json.dumps(raw.model_dump(), ensure_ascii=False)
        problems = v.problems + v.uncovered
        # Uncovered items kept to the last round make the request out of scope, whatever else that plan got wrong.
        unsupported = bool(v.uncovered)
    raise PlanFailed(problems, unsupported)

