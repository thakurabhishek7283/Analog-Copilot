// The tutor evals' simulation values, made the way the editor makes them (LLD §9, §10): each circuit
// snapshot goes into the editor's own store, is simulated by its simulation scheduler on ngspice.wasm
// with the core's spec checks, and `simValues` turns the result into what a question sends. Driven by
// evals/tutor/fixtures.py, which freezes the values into fixtures.json.
// usage: node evals/tutor/sim_values.mts <snapshots.json> <values.json>   (a JSON list of snapshot strings)
import { readdirSync, readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import createNgspice from "../../third_party/ngspice/dist/wasm/ngspice.mjs";
import { bundle, bundleJson, loadCore, repo } from "../../apps/web/src/test/artifacts.ts";
import { createCircuitStore } from "../../apps/web/src/store/circuitStore.ts";
import { attachSimulation } from "../../apps/web/src/store/simulation.ts";
import { simValues } from "../../apps/web/src/tutor/simValues.ts";
import { Ngspice, loadModels, simulate } from "../../apps/web/src/workers/sim.engine.ts";

const [snapshotsPath, valuesPath] = process.argv.slice(2);
const snapshots: string[] = JSON.parse(readFileSync(snapshotsPath!, "utf8"));

const core = loadCore();
const registry = core.CoreRegistry.fromJson(bundleJson());
const reg = bundle();
const ng = new Ngspice(await createNgspice());
const models = join(repo, "registry/models");
loadModels(ng, "/registry", Object.fromEntries(readdirSync(models).map((f) => [`models/${f}`, readFileSync(join(models, f), "utf8")])));

const settled = (status: string) => !["pending", "running"].includes(status);

const values = [];
for (const snapshot of snapshots) {
  const session = new core.CoreSession(registry, snapshot);
  const store = createCircuitStore(session);
  const detach = attachSimulation(store, session, { run: async (req) => simulate(ng, req) }, {
    debounceMs: 0,
    evaluateChecks: core.evaluateChecks,
  });
  const deadline = Date.now() + 30_000;
  while (!settled(store.getState().sim.status)) {
    if (Date.now() > deadline) throw new Error("simulation did not settle in 30 s");
    await new Promise((r) => setTimeout(r, 5));
  }
  detach();
  values.push(simValues(store.getState(), reg));
}
writeFileSync(valuesPath!, JSON.stringify(values));
