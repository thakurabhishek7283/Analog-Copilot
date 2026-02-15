// The simulations "What changed?" compares: one record each time the simulation settles on a rev,
// and a change from the newest record before a rev, named by the edits in between.
import { describe, expect, it } from "vitest";
import type { Inserted } from "../gen/contract.ts";
import { createCircuitStore } from "../store/circuitStore.ts";
import { bundle, bundleJson, loadCore, missingArtifacts } from "../test/artifacts.ts";
import { MAX_RECORDS, changeAt, changeName, createSimHistory } from "./simHistory.ts";

const missing = missingArtifacts();

function setup() {
  const core = loadCore();
  const session = new core.CoreSession(core.CoreRegistry.fromJson(bundleJson()), null);
  const store = createCircuitStore(session);
  const ins = JSON.parse(session.insertBlock(JSON.stringify({ template: "rc_lowpass" }))).ok as Inserted;
  store.getState().applyBatch(ins.ops, "Insert RC", "template");
  return { store, sims: createSimHistory(store, bundle()) };
}

const setR1 = (value: string) => ({ op: "part.set_param" as const, body: { refdes: "R1", key: "resistance", value } });

/** The simulation of the circuit as it is now: pending, then settled with B1_OUT at `v`. */
function simulate(store: ReturnType<typeof setup>["store"], v: number) {
  store.getState().setSim((s) => {
    s.status = "pending";
  });
  store.getState().setSim((s) => Object.assign(s, { status: "ok", view: { op: { v: { B1_OUT: v }, i: {} } } }));
}

describe.skipIf(missing.length > 0)("simulation history", () => {
  it("records each settled simulation with its rev, and names a change by the edits in between", () => {
    const { store, sims } = setup();
    const start = store.getState().rev;
    simulate(store, 0.5);
    store.getState().apply(setR1("2k"), "R1 resistance → 2k");
    expect(sims.getState().records.map((r) => r.rev)).toEqual([start]); // a rev change alone records nothing
    simulate(store, 0.25);

    const c = changeAt(sims.getState().records, store.getState().changes)!;
    expect(c).toMatchObject({ fromRev: start, rev: start + 1, labels: ["R1 resistance → 2k"] });
    expect([c.before.op_v, c.after.op_v]).toEqual([{ B1_OUT: 0.5 }, { B1_OUT: 0.25 }]);
    expect(changeName(c)).toBe("R1 resistance → 2k");

    // Two edits before the simulation settles again are one change; an undo is named as one.
    store.getState().apply(setR1("3k"), "R1 resistance → 3k");
    store.getState().undo();
    simulate(store, 0.25);
    const both = changeAt(sims.getState().records, store.getState().changes)!;
    expect(both).toMatchObject({ fromRev: start + 1, rev: start + 3, labels: ["R1 resistance → 3k", "Undo R1 resistance → 3k"] });
    expect(changeName(both)).toBe("your last 2 changes");
    expect(changeAt(sims.getState().records, store.getState().changes, start + 1)!.fromRev).toBe(start);
    expect(changeAt(sims.getState().records, store.getState().changes, start)).toBeNull(); // nothing before it
  });

  it("keeps the last few records and stops with the project", () => {
    const { store, sims } = setup();
    for (let i = 0; i < MAX_RECORDS + 3; i++) {
      store.getState().apply(setR1(`${i + 1}k`));
      simulate(store, i);
    }
    expect(sims.getState().records).toHaveLength(MAX_RECORDS);
    sims.dispose();
    store.getState().apply(setR1("99k"));
    simulate(store, 9);
    expect(sims.getState().records.at(-1)!.rev).toBe(store.getState().rev - 1);
  });
});
