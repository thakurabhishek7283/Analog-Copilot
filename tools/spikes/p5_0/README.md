# P5.0 spikes (2026-10-11)

Throwaway harness behind the numbers in LLD §17 ("As built, P5.0"). Not part of CI.

| File | What |
| --- | --- |
| `decks.py` | Spike decks: a reduced three-band analyser emitted flat, as value-specialised `.subckt`s and with `params:`; behavioral op-amp (hard/smooth limits, with and without a bounded state) in a Schmitt trigger; a VCA; a 2nd-order transfer function as a state space; the expression functions |
| `scale.py` | Nested RC ladders with behavioral buffers, 210 to 4,200 devices, flat and hierarchical |
| `digital.cir` | XSPICE: 4-bit ripple counter, half adder, R-2R DAC |
| `run_native.py` | Runs a requests file on the native driver (Windows, or the sim-runner image for Linux) |
| `run_xspice.mts` | Runs a requests file on any ngspice WASM build through `sim.engine.ts`, loading code models first |
| `compare.py`, `dump_benches.py` | Cross-engine comparison; the 100 template benches as CI builds them |
| `xspice/` | ngspice 47 WASM with XSPICE and statically linked code models (`build.sh` runs in the pinned emsdk image) |

```sh
python decks.py out/requests.json
PYTHONPATH=workers/sim_runner python run_native.py out/requests.json out/results_win.json
REPO=$PWD node tools/spikes/p5_0/run_xspice.mts third_party/ngspice/dist/wasm/ngspice.mjs out/requests.json out/results_wasm.json
docker run --rm -e HOME=/tmp -e EM_CACHE=/tmp/emcache -v "$PWD/third_party/ngspice:/ngspice:ro" -v "$PWD/tools/spikes/p5_0/xspice:/spike" emscripten/emsdk:6.0.10 bash /spike/build.sh
```
