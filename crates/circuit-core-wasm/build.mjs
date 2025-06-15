// Build the npm packages for circuit-core's WASM facade.
//   node crates/circuit-core-wasm/build.mjs          -> pkg/       (@tutor/core, --target web, for Vite)
//   node crates/circuit-core-wasm/build.mjs nodejs   -> pkg-node/  (CommonJS, for tests and parity)
import { spawnSync } from "node:child_process";
import { readFileSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const crate = dirname(fileURLToPath(import.meta.url));
const target = process.argv[2] ?? "web";
const outDir = { web: "pkg", nodejs: "pkg-node" }[target] ?? `pkg-${target}`;
const r = spawnSync(
  "wasm-pack",
  ["--quiet", "build", crate, "--release", "--target", target, "--out-dir", outDir, "--out-name", "core"],
  { stdio: "inherit" },
);
if (r.status !== 0) process.exit(r.status ?? 1);

const pkgJson = join(crate, outDir, "package.json");
const pkg = JSON.parse(readFileSync(pkgJson, "utf8"));
pkg.name = "@tutor/core";
pkg.private = true;
writeFileSync(pkgJson, JSON.stringify(pkg, null, 2) + "\n");
console.log(`built ${pkg.name}@${pkg.version} (${target}) in ${join("crates/circuit-core-wasm", outDir)}`);
