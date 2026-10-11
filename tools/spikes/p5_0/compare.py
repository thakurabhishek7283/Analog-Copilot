"""Compare .meas results across engines and across the analyser's emission variants."""

import json
import sys
from pathlib import Path

d = Path(sys.argv[1])
reqs = json.loads((d / "requests.json").read_text(encoding="utf-8"))
win = {r["name"]: r for r in json.loads((d / "results_win.json").read_text(encoding="utf-8"))}
lin = {r["name"]: r for r in json.loads((d / "results_linux.json").read_text(encoding="utf-8"))}
wasm_raw = json.loads((d / "results_wasm_raw.json").read_text(encoding="utf-8"))
wasm = {q["name"]: {"status": r["status"], "meas": r["meas"], "ms": r["ms"]} for q, r in zip(reqs, wasm_raw)}


def rel(a: float, b: float) -> float:
    return abs(a - b) / max(abs(a), abs(b), 1e-12)


print(f"{'deck':24} {'status w/l/x':22} {'ms win':>7} {'ms lin':>7} {'ms wasm':>8} {'max rel diff vs win':>20}")
for q in reqs:
    n = q["name"]
    w, l, x = win[n], lin[n], wasm[n]
    worst = 0.0
    missing = []
    for k, v in w["meas"].items():
        for other in (l, x):
            if k not in other["meas"]:
                missing.append(k)
            else:
                worst = max(worst, rel(v, other["meas"][k]))
    print(f"{n:24} {w['status']+'/'+l['status']+'/'+x['status']:22} {w['ms']:7.0f} {l['ms']:7.0f} {x['ms']:8.0f} {worst:20.2e}"
          + (f"  missing {sorted(set(missing))}" if missing else ""))

print("\nanalyser variants, max rel diff vs flat, per engine:")
for eng_name, eng in (("win", win), ("linux", lin), ("wasm", wasm)):
    flat = eng["analyser_flat"]["meas"]
    for v in ("mono", "param"):
        m = eng[f"analyser_{v}"]["meas"]
        print(f"  {eng_name:6} {v:6} {max(rel(flat[k], m[k]) for k in flat):.2e} ({len(flat)} values)")
print("\nnetlist sizes (chars):", {q["name"]: len(q["netlist"]) for q in reqs if q["name"].startswith("analyser")})
