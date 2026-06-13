// The learner's simulation as the tutor's context needs it (LLD §5, §9): the server never simulated
// an edited circuit, so the browser sends what it has, and circuit-core picks the slice's values.
// Built from the store's latest result only once it is for the current circuit (`simSettled`).
import type { AcValues, Registry, SimValues, TranValues } from "../gen/contract.ts";
import type { CircuitState, CircuitStore } from "../store/circuitStore.ts";

const finite = (x: number | undefined): x is number => x !== undefined && Number.isFinite(x);

/** Only finite numbers: JSON has no NaN or Infinity (they would arrive as null and be refused). */
function numbers(record: Record<string, number>): Record<string, number> {
  return Object.fromEntries(Object.entries(record).filter(([, x]) => finite(x)));
}

/** The frequency the learner is driving the circuit at: its slowest sine source (the same rule
 * the core uses for the default transient). Null without one. */
export function signalHz(state: Pick<CircuitState, "parts">, registry: Registry): number | null {
  let hz: number | null = null;
  for (const inst of Object.values(state.parts)) {
    const def = inst && registry.parts[inst.part];
    if (!inst || def?.category !== "V") continue;
    for (const [key, pd] of Object.entries(def.params ?? {})) {
      const f = inst.params[key]?.si;
      if (pd?.unit === "hertz" && finite(f) && f > 0 && (hz === null || f < hz)) hz = f;
    }
  }
  return hz;
}

/** AC magnitude and phase at `hz`, the sweep's nearest point (by ratio), if the sweep covers it. */
function acAt(view: NonNullable<CircuitState["sim"]["view"]>["ac"], hz: number | null): AcValues | null {
  if (!view || hz === null || view.x.length === 0) return null;
  const xs = view.x;
  if (hz < xs[0]! * 0.999 || hz > xs[xs.length - 1]! * 1.001) return null;
  let k = 0;
  for (let i = 1; i < xs.length; i++) if (Math.abs(Math.log(xs[i]! / hz)) < Math.abs(Math.log(xs[k]! / hz))) k = i;
  const v: Record<string, number> = {};
  const deg: Record<string, number> = {};
  for (const [net, re] of Object.entries(view.v)) {
    const r = re[k]!;
    const im = view.vi[net]?.[k] ?? 0;
    v[net] = Math.hypot(r, im);
    deg[net] = (Math.atan2(im, r) * 180) / Math.PI;
  }
  return { hz: xs[k]!, v: numbers(v), deg: numbers(deg) };
}

/** Each net's lowest and highest voltage over the transient's second half (after start-up). */
export function tranRange(view: NonNullable<CircuitState["sim"]["view"]>["tran"]): TranValues | null {
  if (!view || view.x.length < 2) return null;
  const from = Math.floor(view.x.length / 2);
  const v: TranValues["v"] = {};
  for (const [net, ys] of Object.entries(view.v)) {
    let min = Infinity;
    let max = -Infinity;
    for (let i = from; i < ys.length; i++) {
      const y = ys[i]!;
      if (y < min) min = y;
      if (y > max) max = y;
    }
    if (finite(min) && finite(max)) v[net] = { min, max };
  }
  return { t_stop: view.x[view.x.length - 1]!, v };
}

export function simValues(state: Pick<CircuitState, "sim" | "parts">, registry: Registry): SimValues {
  const { sim } = state;
  if (sim.status === "idle") return {}; // nothing to simulate
  const status = sim.result?.status ?? (sim.status === "error" ? "error" : undefined);
  const out: SimValues = { ...(status ? { status } : {}), op_v: {}, op_i: {}, checks: sim.checks ?? [] };
  const view = sim.view;
  if (!view) return out;
  if (view.op) {
    out.op_v = numbers(view.op.v);
    out.op_i = numbers(view.op.i);
  }
  const ac = acAt(view.ac, signalHz(state, registry));
  if (ac) out.ac = ac;
  const tran = tranRange(view.tran);
  if (tran) out.tran = tran;
  return out;
}

/** Resolves once the simulation is for the circuit as it is now (not pending or running), or
 * after `timeoutMs`: a question waits for the values of the circuit it is about. */
export function simSettled(store: CircuitStore, timeoutMs = 5000): Promise<void> {
  const busy = () => ["pending", "running"].includes(store.getState().sim.status);
  return new Promise((resolve) => {
    if (!busy()) return resolve();
    const done = () => {
      clearTimeout(timer);
      unsubscribe();
      resolve();
    };
    const timer = setTimeout(done, timeoutMs);
    const unsubscribe = store.subscribe(() => {
      if (!busy()) done();
    });
  });
}
