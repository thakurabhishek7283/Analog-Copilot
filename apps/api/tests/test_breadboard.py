"""The physical guide must preserve the verified schematic's exact pin partitions."""

from copy import deepcopy
import json
from pathlib import Path

import circuit_core as cc
import pytest

from tutor_api import breadboard as bb


def registry():
    def pins(*names):
        return [{"name": name, "num": n} for n, name in enumerate(names, 1)]

    return {"parts": {
        "vsource_sine": {"pins": pins("P", "N")},
        "resistor_th": {"pins": pins("1", "2")},
        "cap_film": {"pins": pins("1", "2")},
    }}


def circuit():
    return {"rev": 12, "parts": {
        "V1": {"refdes": "V1", "part": "vsource_sine"},
        "R1": {"refdes": "R1", "part": "resistor_th"},
        "C1": {"refdes": "C1", "part": "cap_film"},
    }, "nets": {
        "IN": {"pins": ["V1.P", "R1.1"]},
        "OUT": {"pins": ["R1.2", "C1.1"]},
        "GND": {"pins": ["V1.N", "C1.2"]},
    }}


def candidate():
    return bb.Candidate(boards=2, placements=[
        bb.CandidatePart(refdes="R1", board=1, row=5),
        bb.CandidatePart(refdes="C1", board=2, row=7),
    ])


def test_routes_two_boards_and_exact_pin_nets():
    layout = bb.assemble(candidate(), circuit(), registry())
    assert layout.parts["R1"].holes == {"1": "B1:E5", "2": "B1:F5"}
    assert layout.parts["C1"].holes == {"1": "B2:E7", "2": "B2:E8"}
    assert len(layout.jumpers) == 3
    assert any({w.from_hole[:2], w.to[:2]} == {"B1", "B2"} for w in layout.jumpers)
    assert bb.validate(layout, circuit(), registry()) == []


def test_rejects_missing_wire_short_and_wrong_footprint():
    layout = bb.assemble(candidate(), circuit(), registry())
    broken = layout.model_copy(deep=True)
    broken.jumpers.pop()
    assert "connections differ" in " ".join(bb.validate(broken, circuit(), registry()))

    short = layout.model_copy(deep=True)
    short.jumpers.append(bb.Jumper(id="J4", **{"from": "B1:A5", "to": "B1:G5"}))
    assert bb.validate(short, circuit(), registry())

    moved = layout.model_copy(deep=True)
    moved.parts["C1"].holes["1"] = "B2:E9"
    assert "footprint" in " ".join(bb.validate(moved, circuit(), registry()))


def test_rejects_overlap_and_unlisted_package():
    overlapping = bb.Candidate(boards=1, placements=[
        bb.CandidatePart(refdes="R1", board=1, row=5),
        bb.CandidatePart(refdes="C1", board=1, row=5),
    ])
    with pytest.raises(ValueError, match="overlap"):
        bb.assemble(overlapping, circuit(), registry())
    unsupported = deepcopy(circuit())
    unsupported["parts"]["Q1"] = {"refdes": "Q1", "part": "unknown_surface_mount"}
    assert bb.unsupported(unsupported) == ["Q1 (unknown_surface_mount)"]


def test_curated_footprints_match_the_current_registry_pin_counts():
    source = Path(__file__).resolve().parents[3] / "registry"
    registry = json.loads(cc.load_registry_dir(source).to_json())
    for part_id, (_, offsets) in bb.FOOTPRINTS.items():
        assert part_id in registry["parts"]
        assert len(offsets) == len(registry["parts"][part_id]["pins"]), part_id
    assert not bb.unsupported({"parts": {name: {"part": name} for name in registry["parts"]}})
