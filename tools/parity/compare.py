"""Fail unless native, WASM and Python produced identical digests for every log."""

import json
import sys
from pathlib import Path

parity_dir = Path(sys.argv[1])
runs = {name: json.loads((parity_dir / f"{name}.json").read_text())["digests"] for name in ("native", "wasm", "python")}
n = len(runs["native"])
bad = [i for i in range(n) if not (runs["native"][i] == runs["wasm"][i] == runs["python"][i])]
if any(len(d) != n for d in runs.values()):
    sys.exit(f"FAIL: log counts differ: { {k: len(v) for k, v in runs.items()} }")
if bad:
    sys.exit(f"FAIL: {len(bad)} of {n} logs diverge, first at index {bad[0]}")
print(f"PASS: {n} logs identical across native, WASM and Python")
