"""Write the 100 template verification benches (as CI builds them) plus the spike decks as one
requests file. Run from the repo root with tools/sim and workers/sim_runner on PYTHONPATH."""
import json, sys
import test_templates as tt
out = []
for tid, k in tt.POINTS:
    n = tt.build(tid, k)[0].netlist(interactive=True)
    out.append({"name": f"{tid}-{k}", "netlist": n["text"], "includes": n["includes"], "hash": n["hash"], "checks": n["checks"]})
spike = json.load(open(sys.argv[1], encoding="utf-8"))
json.dump(out + spike, open(sys.argv[2], "w", encoding="utf-8"))
print(len(out), "benches +", len(spike), "spike decks")
