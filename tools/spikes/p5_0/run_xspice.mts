// Run decks on the XSPICE spike build (or the current build) through the editor's own engine.
// usage: node run_xspice.mts <module.mjs> <requests.json> <results.json> [codemodels]
import { readFileSync, writeFileSync } from "node:fs";
import { join, resolve } from "node:path";
import { pathToFileURL } from "node:url";
const repo = process.env.REPO!;
const { Ngspice, loadModels, simulate } = await import(pathToFileURL(join(repo, "apps/web/src/workers/sim.engine.ts")).href);
const [modPath, reqPath, outPath, cms] = process.argv.slice(2);
const createNgspice = (await import(pathToFileURL(resolve(modPath!)).href)).default;
const requests = JSON.parse(readFileSync(reqPath!, "utf8"));
const includes = [...new Set(requests.flatMap((r: any) => r.includes))] as string[];
const t0 = performance.now();
const ng = new Ngspice(await createNgspice());
const initMs = performance.now() - t0;
if (cms) for (const cm of cms.split(",")) { ng.command(`codemodel ${cm}`); }
const cmLog = ng.takeLog();
loadModels(ng, "/registry", Object.fromEntries(includes.map((p) => [p, readFileSync(join(repo, "registry", p), "utf8")])));
const out = requests.map((r: any) => {
  const res = simulate(ng, { netlist: r.netlist, hash: r.hash, analyses: [] });
  return { name: r.name, status: res.status, meas: res.meas, ms: res.ms, log: res.status === "ok" ? "" : res.log.slice(-1500) };
});
writeFileSync(outPath!, JSON.stringify({ initMs, cmLog, results: out }, null, 1));
