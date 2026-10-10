"""Optional physical build guide. Nothing here writes to the circuit op log.

The model chooses board/row positions; a curated package map assigns numbered leads to
holes. Routing and validation are deterministic, so model output cannot certify itself.
"""

from __future__ import annotations

import re
from collections import defaultdict
from typing import Any

from pydantic import Field

from .orchestrator.schemas import Strict

ROWS = 63
MAX_BOARDS = 8

# Pin order is the registry's numerical order. Pitch is one 2.54 mm row. Packages are
# assumptions about the *purchased* part, not facts inferred from its SPICE model.
FOOTPRINTS: dict[str, tuple[str, tuple[tuple[str, int], ...]]] = {
    "resistor_th": ("axial 1/4 W, leads bent to 7.62 mm", (("E", 0), ("F", 0))),
    "inductor": ("axial through-hole, leads bent to 7.62 mm", (("E", 0), ("F", 0))),
    "diode_1n4148": ("DO-35 axial, leads bent to 7.62 mm", (("E", 0), ("F", 0))),
    "zener_5v1": ("axial through-hole, leads bent to 7.62 mm", (("E", 0), ("F", 0))),
    "cap_film": ("radial film, 2.5 mm lead pitch", (("E", 0), ("E", 1))),
    "cap_elec": ("radial electrolytic, 2.5 mm lead pitch", (("E", 0), ("E", 1))),
    "led_red": ("5 mm LED, 2.54 mm lead pitch", (("E", 0), ("E", 1))),
    "npn_2n3904": ("TO-92 E-B-C, leads 1–3 at increasing rows; confirm purchased pinout", (("E", 0), ("E", 1), ("E", 2))),
    "pnp_2n3906": ("TO-92 E-B-C, leads 1–3 at increasing rows; confirm purchased pinout", (("E", 0), ("E", 1), ("E", 2))),
    "reg_lm7805": ("TO-220 IN-GND-OUT, leads 1–3 at increasing rows; confirm purchased pinout", (("E", 0), ("E", 1), ("E", 2))),
    "timer_ne555": ("DIP-8, 7.62 mm row spacing", (("E", 0), ("E", 1), ("E", 2), ("E", 3), ("F", 3), ("F", 2), ("F", 1), ("F", 0))),
    "opamp_lm358": ("DIP-8, 7.62 mm row spacing", (("E", 0), ("E", 1), ("E", 2), ("E", 3), ("F", 3), ("F", 2), ("F", 1), ("F", 0))),
    "opamp_tl072": ("DIP-8, 7.62 mm row spacing", (("E", 0), ("E", 1), ("E", 2), ("E", 3), ("F", 3), ("F", 2), ("F", 1), ("F", 0))),
}


class CandidatePart(Strict):
    refdes: str
    board: int = Field(ge=1, le=MAX_BOARDS)
    row: int = Field(ge=1, le=ROWS)


class Candidate(Strict):
    boards: int = Field(ge=1, le=MAX_BOARDS)
    placements: list[CandidatePart] = Field(max_length=150)


class PlacedPart(Strict):
    board: int = Field(ge=0, le=MAX_BOARDS)
    row: int = Field(ge=0, le=ROWS)
    package: str
    holes: dict[str, str]


class Jumper(Strict):
    id: str
    from_hole: str = Field(alias="from")
    to: str


class Layout(Strict):
    rev: int = Field(ge=0)
    boards: int = Field(ge=1, le=MAX_BOARDS)
    parts: dict[str, PlacedPart]
    jumpers: list[Jumper] = Field(max_length=1000)


HOLE = re.compile(r"^B([1-8]):([A-J])([1-9]|[1-5][0-9]|6[0-3])$")


def _pins(registry: Any, part_id: str) -> list[str]:
    return [p["name"] for p in sorted(registry["parts"][part_id]["pins"], key=lambda p: p["num"])]


def _expected(part: dict, registry: Any, board: int, row: int) -> PlacedPart:
    part_id = part["part"]
    pins = _pins(registry, part_id)
    if part_id.startswith("vsource_"):
        return PlacedPart(board=0, row=0, package="external source; connect labelled terminals", holes={p: f"X:{part['refdes']}.{p}" for p in pins})
    package, offsets = FOOTPRINTS[part_id]
    if len(pins) != len(offsets) or row + max(delta for _, delta in offsets) > ROWS:
        raise ValueError(f"{part['refdes']} does not fit its {package} footprint")
    return PlacedPart(board=board, row=row, package=package,
                      holes={pin: f"B{board}:{column}{row + delta}" for pin, (column, delta) in zip(pins, offsets)})


def unsupported(circuit: dict) -> list[str]:
    return [f"{ref} ({part['part']})" for ref, part in circuit["parts"].items()
            if part["part"] not in FOOTPRINTS and not part["part"].startswith("vsource_")]


def _strip(hole: str, boards: int) -> str:
    if hole.startswith("X:"):
        return hole
    match = HOLE.fullmatch(hole)
    if not match or int(match[1]) > boards:
        raise ValueError(f"invalid board hole {hole}")
    return f"B{match[1]}:{'L' if match[2] <= 'E' else 'R'}{match[3]}"


def validate(layout: Layout, circuit: dict, registry: Any) -> list[str]:
    errors: list[str] = []
    if layout.rev != circuit["rev"]:
        errors.append("The 2D circuit revision changed; regenerate the guide.")
    if set(layout.parts) != set(circuit["parts"]):
        errors.append("The guide must contain every schematic part exactly once.")
    occupied_ranges: dict[int, list[tuple[int, int, str]]] = defaultdict(list)
    occupied: set[str] = set()
    for ref, part in circuit["parts"].items():
        placed = layout.parts.get(ref)
        if not placed:
            continue
        try:
            expected = _expected(part, registry, placed.board, placed.row)
            if placed != expected or (placed.board > layout.boards):
                errors.append(f"{ref}: footprint, board, or pin holes do not match the supported package.")
            if placed.board:
                footprint_rows = {int(h.split(":", 1)[1][1:]) for h in expected.holes.values()}
                first, last = min(footprint_rows), max(footprint_rows)
                for other_first, other_last, other_ref in occupied_ranges[placed.board]:
                    if first <= other_last + 1 and last >= other_first - 1:
                        errors.append(f"{ref}: package overlaps or has no blank row beside {other_ref} on board {placed.board}.")
                occupied_ranges[placed.board].append((first, last, ref))
            for hole in placed.holes.values():
                _strip(hole, layout.boards)
                if hole in occupied:
                    errors.append(f"{ref}: hole {hole} already contains a lead.")
                occupied.add(hole)
        except (KeyError, ValueError) as exc:
            errors.append(f"{ref}: {exc}")
    parent: dict[str, str] = {}

    def root(key: str) -> str:
        parent.setdefault(key, key)
        if parent[key] != key:
            parent[key] = root(parent[key])
        return parent[key]

    def join(a: str, b: str) -> None:
        ra, rb = root(a), root(b)
        if ra != rb:
            parent[ra] = rb

    endpoints: set[str] = set()
    external = {hole for hole in occupied if hole.startswith("X:")}
    for wire in layout.jumpers:
        try:
            a, b = _strip(wire.from_hole, layout.boards), _strip(wire.to, layout.boards)
            if any(hole.startswith("X:") and hole not in external for hole in (wire.from_hole, wire.to)):
                errors.append(f"{wire.id}: external terminal is not part of the circuit.")
            if any(hole in occupied or hole in endpoints for hole in (wire.from_hole, wire.to) if not hole.startswith("X:")):
                errors.append(f"{wire.id}: a hole cannot hold two leads or jumper ends.")
            endpoints.update((wire.from_hole, wire.to))
            join(a, b)
        except ValueError as exc:
            errors.append(f"{wire.id}: {exc}")
    if len({w.id for w in layout.jumpers}) != len(layout.jumpers):
        errors.append("Jumper identifiers must be unique.")
    if errors:
        return errors
    actual: dict[str, list[str]] = defaultdict(list)
    for ref, placed in layout.parts.items():
        for pin, hole in placed.holes.items():
            actual[root(_strip(hole, layout.boards))].append(f"{ref}.{pin}")
    for wire in layout.jumpers:
        if root(_strip(wire.from_hole, layout.boards)) not in actual:
            errors.append(f"{wire.id}: jumper is not connected to any circuit pin.")
    all_pins = {pin for group in actual.values() for pin in group}
    expected_groups = [sorted(net["pins"]) for net in circuit["nets"].values() if net["pins"]]
    assigned = {pin for group in expected_groups for pin in group}
    expected_groups.extend([pin] for pin in sorted(all_pins - assigned))
    if sorted(map(tuple, map(sorted, actual.values()))) != sorted(map(tuple, expected_groups)):
        errors.append("Breadboard connections differ from the verified 2D circuit (missing link or short).")
    return errors


def assemble(candidate: Candidate, circuit: dict, registry: Any) -> Layout:
    placements = {p.refdes: p for p in candidate.placements}
    if len(placements) != len(candidate.placements) or set(placements) != {r for r, p in circuit["parts"].items() if not p["part"].startswith("vsource_")}:
        raise ValueError("Provide one placement for every non-source part.")
    parts = {ref: _expected(part, registry, placements[ref].board, placements[ref].row)
             if ref in placements else _expected(part, registry, 0, 0) for ref, part in circuit["parts"].items()}
    # Each pin strip has four spare holes. A chain needs at most two jumper ends per pin.
    taken = {hole for placed in parts.values() for hole in placed.holes.values()}

    def free(lead: str) -> str:
        if lead.startswith("X:"):
            return lead
        prefix, column, row = lead.split(":")[0], lead.split(":")[1][0], lead.split(":")[1][1:]
        for col in ("ABCD" if column <= "E" else "GHIJ"):
            hole = f"{prefix}:{col}{row}"
            if hole not in taken:
                taken.add(hole)
                return hole
        raise ValueError(f"No spare hole near {lead} for a jumper.")

    wires: list[Jumper] = []
    for net in circuit["nets"].values():
        pins = sorted(net["pins"])
        for a, b in zip(pins, pins[1:]):
            ra, pa = a.split(".", 1)
            rb, pb = b.split(".", 1)
            wires.append(Jumper(id=f"J{len(wires)+1}", **{"from": free(parts[ra].holes[pa]), "to": free(parts[rb].holes[pb])}))
    layout = Layout(rev=circuit["rev"], boards=candidate.boards, parts=parts, jumpers=wires)
    problems = validate(layout, circuit, registry)
    if problems:
        raise ValueError("; ".join(problems[:5]))
    return layout
