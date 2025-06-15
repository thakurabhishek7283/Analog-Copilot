// Cross-runtime parity, WASM side: fold every log in logs.json through @tutor/core (Node build)
// and write per-log digests. Mirrors `digest()` in crates/circuit-core/examples/parity_gen.rs.
// usage: node tools/parity/run_node.mjs <parity_dir> <registry_bundle.json> <pkg_dir>
import { createHash } from "node:crypto";
import { readFileSync, writeFileSync } from "node:fs";
import { createRequire } from "node:module";
import { resolve } from "node:path";

const [dir, bundlePath, pkgDir] = process.argv.slice(2);
const core = createRequire(import.meta.url)(resolve(pkgDir, "core.js"));
const registry = core.CoreRegistry.fromJson(readFileSync(bundlePath, "utf8"));
const { logs } = JSON.parse(readFileSync(`${dir}/logs.json`, "utf8"));

const digests = logs.map((log) => {
  const s = new core.CoreSession(registry);
  const out = log.map((env) => s.apply(env));
  out.push(s.snapshot());
  out.push(s.compile("{}"));
  out.push(s.compile('{"shunt_floating":true}'));
  out.push(s.erc("user_edit", undefined));
  out.push(s.erc("llm_block", "b1"));
  out.push(s.circuitText());
  const fresh = new core.CoreSession(registry);
  out.push(fresh.applyAll(`[${log.filter((e) => e.endsWith("}")).join(",")}]`));
  s.free();
  fresh.free();
  return createHash("sha256").update(out.join("\n"), "utf8").digest("hex");
});
writeFileSync(`${dir}/wasm.json`, JSON.stringify({ runtime: "wasm", digests }));
console.log(`wasm: ${digests.length} logs`);
