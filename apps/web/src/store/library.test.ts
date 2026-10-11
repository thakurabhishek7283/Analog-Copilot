// Library components in the editor (LLD §17): the store mirrors the circuit's library and its
// generated symbols through the core's patches, and the layout draws an instance from them.
import ELK from "elkjs/lib/elk.bundled.js";
import { describe, expect, it } from "vitest";
import type { Op } from "../gen/contract.ts";
import { bundle, bundleJson, demoJson, loadCore, missingArtifacts } from "../test/artifacts.ts";
import { LayoutEngine } from "../workers/layout.engine.ts";
import { layoutRegistry, toLayoutInput } from "../workers/layoutClient.ts";
import { createCircuitStore } from "./circuitStore.ts";

const missing = missingArtifacts();

const pin = (name: string, num: number, type: string) => ({ name, num, type });
const buffer = {
  part: {
    id: "lib:buf",
    category: "X",
    prefix: "U",
    title: "Buffer",
    symbol: "auto",
    pins: [pin("IN", 1, "input"), pin("OUT", 2, "output"), pin("VCC", 3, "power_pos"), pin("VEE", 4, "power_neg")],
  },
  version: 1,
  model: {
    kind: "composite",
    parts: { U1: { part: "opamp_tl072" } },
    nets: { IN: ["U1.INP_A"], OUT: ["U1.OUT_A", "U1.INM_A"], VCC: ["U1.VCC"], VEE: ["U1.VEE"] },
  },
  provenance: { kind: "user" },
};

function setup(snapshot: string | null = demoJson()) {
  const core = loadCore();
  const session = new core.CoreSession(core.CoreRegistry.fromJson(bundleJson()), snapshot);
  return createCircuitStore(session);
}

describe.skipIf(missing.length > 0)("library components", () => {
  it("mirrors the library and its symbols, lays out an instance, and undoes", async () => {
    const store = setup();
    expect(store.getState().library).toEqual({});
    const ops = [
      { op: "lib.add", body: { def: buffer } },
      { op: "part.add", body: { refdes: "U2", part: "lib:buf" } },
      { op: "net.connect", body: { net: "N_OUT", pins: ["U2.IN"] } },
      { op: "net.connect", body: { net: "N_BUF", pins: ["U2.OUT"] } },
      { op: "net.connect", body: { net: "VCC", pins: ["U2.VCC"] } },
      { op: "net.connect", body: { net: "VEE", pins: ["U2.VEE"] } },
    ] as unknown as Op[];
    const r = store.getState().applyBatch(ops, "Add a buffer");
    expect(r.err).toBeUndefined();
    const s = store.getState();
    expect(Object.keys(s.library)).toEqual(["lib:buf"]);
    expect(s.library["lib:buf"]!.part.pins.map((p) => p.dc_group ?? null)).toEqual([null, 1, 1, 1]);
    const sym = s.librarySymbols["lib:buf"]!;
    expect(sym.id).toBe("lib_buf");
    expect(sym.def.pins.IN!.side).toBe("left");
    expect(sym.def.pins.OUT!.side).toBe("right");
    expect(sym.body).toContain("<rect");

    const input = toLayoutInput(s);
    expect(input.library?.parts["lib:buf"]?.symbol).toBe("lib_buf");
    const layout = await new LayoutEngine(layoutRegistry(bundle()), new ELK()).layout(input);
    const placed = layout.symbols.U2!;
    expect(placed.symbol).toBe("lib_buf");
    expect([placed.symbolWidth, placed.symbolHeight]).toEqual([sym.def.width, sym.def.height]);
    expect(layout.pins["U2.IN"]).toHaveLength(1);
    expect(layout.flags.some((f) => f.key === "flag:U2.VCC")).toBe(true);

    expect(store.getState().undo()).toBe(true);
    expect(store.getState().library).toEqual({});
    expect(store.getState().librarySymbols).toEqual({});
    expect(store.getState().parts.U2).toBeUndefined();
  });

  it("loads a snapshot that already has a library", () => {
    const first = setup();
    first.getState().applyBatch([{ op: "lib.add", body: { def: buffer } }] as unknown as Op[], "Define");
    const core = loadCore();
    const snap = JSON.stringify({ ...JSON.parse(demoJson()), library: first.getState().library, schema_version: 2 });
    const session = new core.CoreSession(core.CoreRegistry.fromJson(bundleJson()), snap);
    const store = createCircuitStore(session);
    expect(Object.keys(store.getState().library)).toEqual(["lib:buf"]);
    expect(store.getState().librarySymbols["lib:buf"]!.id).toBe("lib_buf");
  });
});
