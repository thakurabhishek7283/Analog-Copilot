#!/usr/bin/env bash
# Cross-runtime parity gate (LLD §15): the same op logs must give identical results in native
# Rust, WASM (Node) and PyO3 (Python). Needs: cargo, wasm-pack, node, uv.
# usage: tools/parity/run.sh [count] [seed]
set -euo pipefail
cd "$(dirname "$0")/../.."
COUNT="${1:-1000}"
SEED="${2:-1592297166}"
OUT=target/parity
PY=.venv/Scripts/python; [ -x "$PY" ] || PY=.venv/bin/python

[ -x "$PY" ] || uv venv -q --python 3.12 .venv
uv pip install -q --python .venv maturin
VIRTUAL_ENV="$PWD/.venv" uv run --no-project --python .venv maturin develop -q --release -m crates/circuit-core-py/Cargo.toml
node crates/circuit-core-wasm/build.mjs nodejs

BUNDLE=$(cargo run -q --release -p circuit-core --example bundle_registry)
cargo run -q --release -p circuit-core --example parity_gen -- "$OUT" "$COUNT" "$SEED"
node tools/parity/run_node.mjs "$OUT" "$BUNDLE" crates/circuit-core-wasm/pkg-node
"$PY" tools/parity/run_python.py "$OUT" "$BUNDLE"
"$PY" tools/parity/compare.py "$OUT"
