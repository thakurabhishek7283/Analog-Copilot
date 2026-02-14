// What the browser sends the tutor of its simulation: finite numbers only, AC at the frequency the
// circuit is driven at (only where the sweep covers it), transient ranges after start-up.
import { describe, expect, it } from "vitest";
import type { PartInstance, Registry } from "../gen/contract.ts";
import type { SimState } from "../store/circuitStore.ts";
import type { SimView } from "../store/simView.ts";
import { signalHz, simValues } from "./simValues.ts";

const registry = {
  version: "t",
  symbols: {},
  parts: {
    vsource_sine: { id: "vsource_sine", category: "V", title: "Sine", pins: [], params: { frequency: { unit: "hertz", default: "1k" } } },
    resistor_th: { id: "resistor_th", category: "R", title: "R", pins: [], params: { resistance: { unit: "ohm", default: "1k" } } },
  },
} as unknown as Registry;

const part = (refdes: string, kind: string, params: Record<string, number>): PartInstance =>
  ({ refdes, part: kind, params: Object.fromEntries(Object.entries(params).map(([k, si]) => [k, { si, unit: "hertz", display: "" }])), origin: { kind: "user" } }) as unknown as PartInstance;

const f64 = (xs: number[]) => Float64Array.from(xs);

const view: SimView = {
  op: { v: { N_IN: 0, N_OUT: Number.NaN }, i: { "R1.1": 1e-3, "R1.2": Number.POSITIVE_INFINITY } },
  // 10 Hz .. 100 kHz, one point a decade.
  ac: { x: f64([10, 100, 1e3, 1e4, 1e5]), v: { N_OUT: f64([1, 1, 0.6, 0.1, 0.01]) }, vi: { N_OUT: f64([0, 0, -0.6, 0, 0]) } },
  // A start-up overshoot to 3 V, then a 1 V swing.
  tran: { x: f64([0, 1, 2, 3, 4, 5]), v: { N_OUT: f64([0, 3, 0.2, -1, 1, -1]) }, i: {} },
};

const sim = (s: Partial<SimState>): SimState => ({ hash: "h", status: "ok", voltages: {}, ...s });
const result = { hash: "h", vectors: [], meas: {}, status: "ok" as const, log: "", ms: 1 };

describe("simValues", () => {
  it("sends finite values, AC at the source's frequency and the transient after start-up", () => {
    const parts = { V1: part("V1", "vsource_sine", { frequency: 900 }), R1: part("R1", "resistor_th", { resistance: 1e3 }) };
    const out = simValues({ parts, sim: sim({ result, view }) }, registry);
    expect(out.status).toBe("ok");
    expect(out.op_v).toEqual({ N_IN: 0 });
    expect(out.op_i).toEqual({ "R1.1": 1e-3 });
    expect(out.ac!.hz).toBe(1000); // the sweep's point nearest 900 Hz
    expect(out.ac!.v.N_OUT).toBeCloseTo(Math.hypot(0.6, 0.6));
    expect(out.ac!.deg!.N_OUT).toBeCloseTo(-45);
    expect(out.tran).toEqual({ t_stop: 5, v: { N_OUT: { min: -1, max: 1 } } });
    expect(out.checks).toEqual([]);
  });

  it("leaves AC out when nothing drives the circuit inside the sweep, and sends nothing before a simulation", () => {
    const outside = { V1: part("V1", "vsource_sine", { frequency: 1e6 }) };
    expect(simValues({ parts: outside, sim: sim({ result, view }) }, registry).ac).toBeUndefined();
    expect(simValues({ parts: {}, sim: sim({ result, view }) }, registry).ac).toBeUndefined();
    expect(simValues({ parts: {}, sim: sim({ status: "idle" }) }, registry)).toEqual({});
    expect(simValues({ parts: {}, sim: sim({ status: "error", message: "R1: no value" }) }, registry)).toMatchObject({ status: "error" });
  });

  it("drives at the slowest sine source", () => {
    const parts = { V1: part("V1", "vsource_sine", { frequency: 5e3 }), V2: part("V2", "vsource_sine", { frequency: 50 }) };
    expect(signalHz({ parts }, registry)).toBe(50);
  });
});
