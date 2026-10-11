"""Scale decks: a chain of RC sections, 3 levels deep (stage -> group of 10 -> section), with a
behavioral buffer (bounded-state op-amp) after every group, flat and hierarchical.
Writes scale_requests.json. Leaf devices = n_groups * (10 sections * 2 + buffer)."""

import json
import sys

from decks import bopamp


def deck(n_groups: int, hier: bool) -> str:
    lines = ["* p50 scale", *bopamp(False)]
    if hier:
        lines += [".subckt SEC a b", "r1 a b 1k", "c1 b 0 1n", ".ends SEC"]
        grp = [".subckt GRP in out vcc"]
        for i in range(10):
            grp.append(f"x{i} n{i} n{i + 1} SEC")
        grp += ["xb n10 out vcc 0 out BOPAMP_H", "rin in n0 1", ".ends GRP"]
        lines += grp
        for g in range(n_groups):
            lines.append(f"xg{g} s{g} s{g + 1} vcc GRP")
    else:
        for g in range(n_groups):
            lines.append(f"rg{g}_in s{g} g{g}_n0 1")
            for i in range(10):
                lines += [f"rg{g}_{i} g{g}_n{i} g{g}_n{i + 1} 1k", f"cg{g}_{i} g{g}_n{i + 1} 0 1n"]
            lines.append(f"xg{g}_b g{g}_n10 s{g + 1} vcc 0 s{g + 1} BOPAMP_H")
    lines += [
        "VCC vcc 0 DC 5",
        "VIN s0 0 DC 2.5 AC 0.5 SIN(2.5 0.5 1k)",
        ".options reltol=1e-3 gmin=1e-12",
        ".op",
        ".tran 5u 5m",
        f".meas tran out_pp pp v(s{n_groups}) from=2.5m to=5m",
        f".meas tran out_avg avg v(s{n_groups}) from=2.5m to=5m",
        ".end", "",
    ]
    return "\n".join(lines)


reqs = []
for n in (10, 50, 100, 200):
    for hier in (False, True):
        reqs.append({"name": f"scale_{n * 21}_{'hier' if hier else 'flat'}", "netlist": deck(n, hier),
                     "includes": [], "hash": ""})
json.dump(reqs, open(sys.argv[1], "w", encoding="utf-8"), indent=1)
