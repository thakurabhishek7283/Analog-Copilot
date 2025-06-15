"""Cross-runtime parity, PyO3 side: fold every log in logs.json through circuit_core and write
per-log digests. Mirrors `digest()` in crates/circuit-core/examples/parity_gen.rs.

usage: python tools/parity/run_python.py <parity_dir> <registry_bundle.json>
"""

import hashlib
import json
import sys
from pathlib import Path

import circuit_core

parity_dir, bundle_path = Path(sys.argv[1]), Path(sys.argv[2])
registry = circuit_core.Registry.from_json(bundle_path.read_text(encoding="utf-8"))
logs = json.loads((parity_dir / "logs.json").read_text(encoding="utf-8"))["logs"]

digests = []
for log in logs:
    s = circuit_core.Session(registry)
    out = [s.apply(env) for env in log]
    out.append(s.snapshot())
    out.append(s.compile("{}"))
    out.append(s.compile('{"shunt_floating":true}'))
    out.append(s.erc("user_edit", None))
    out.append(s.erc("llm_block", "b1"))
    out.append(s.circuit_text())
    fresh = circuit_core.Session(registry)
    out.append(fresh.apply_all("[" + ",".join(e for e in log if e.endswith("}")) + "]"))
    digests.append(hashlib.sha256("\n".join(out).encode("utf-8")).hexdigest())

(parity_dir / "python.json").write_text(json.dumps({"runtime": "python", "digests": digests}), encoding="utf-8")
print(f"python: {len(digests)} logs")
