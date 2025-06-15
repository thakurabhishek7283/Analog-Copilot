# Circuit Forge

Spec: [docs/LLD.md](docs/LLD.md).

## Layout (so far)

| Path | What |
| --- | --- |
| `crates/circuit-core` | IR, op protocol, `apply()`, ERC, SPICE compiler. Pure Rust, the single source of truth. |
| `crates/circuit-core-wasm` | wasm-bindgen facade → npm package `@tutor/core` |
| `crates/circuit-core-py` | PyO3 facade → Python package `circuit_core` |
| `contract/schema` | JSON Schemas exported from the Rust types (generated, committed) |
| `apps/web/src/gen`, `apps/api/tutor_api/models` | TS types / Pydantic models generated from the schemas (do not edit) |
| `registry` | Parts (YAML), SPICE models, bundle manifest |
| `tools/parity` | Cross-runtime parity gate: native vs WASM vs Python |
| `tools/codegen` | Schema → TS / Pydantic generation |

## Setup

Needs Rust (with `wasm32-unknown-unknown`), `wasm-pack`, Node 24 and `uv`.

```sh
rustup target add wasm32-unknown-unknown
cargo install --locked wasm-pack
uv venv --python 3.12 .venv && uv pip install --python .venv maturin pytest "pydantic>=2.9,<3"
export PYO3_PYTHON="$PWD/.venv/Scripts/python.exe"   # Windows; .venv/bin/python elsewhere
```

## Everyday commands

```sh
cargo test --workspace                     # core tests (apply/undo properties, ERC, golden netlists)
tools/parity/run.sh                        # builds both bindings, then checks 1000 random op logs
tools/codegen/run.sh                       # schemas -> TS + Pydantic (add --check in CI)
node crates/circuit-core-wasm/build.mjs    # browser package in crates/circuit-core-wasm/pkg
cargo run -p circuit-core --example bundle_registry   # registry JSON bundle in target/registry
```

After changing a wire type: `tools/codegen/run.sh`, then commit the regenerated files.
After a deliberate netlist change: `cargo insta review` (or `INSTA_UPDATE=always cargo test`) and review the diff.
