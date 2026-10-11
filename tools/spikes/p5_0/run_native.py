"""Run requests.json through the native ngspice driver; write results JSON.
usage: python run_native.py <requests.json> <results.json> [repeats]"""

import json
import sys
import time

from sim_runner.ngspice_batch import simulate

reqs = json.load(open(sys.argv[1], encoding="utf-8"))
repeats = int(sys.argv[3]) if len(sys.argv) > 3 else 3
out = []
for r in reqs:
    times = []
    for _ in range(repeats):
        t = time.perf_counter()
        res = simulate(r["netlist"], r["includes"], hash=r["hash"], timeout_s=30, vectors=False)
        times.append((time.perf_counter() - t) * 1e3)
    out.append({"name": r["name"], "status": res.status, "meas": res.meas, "failed": res.failed_meas,
                "ms": sorted(times)[len(times) // 2], "log": res.log[-1500:]})
json.dump(out, open(sys.argv[2], "w", encoding="utf-8"), indent=1)
