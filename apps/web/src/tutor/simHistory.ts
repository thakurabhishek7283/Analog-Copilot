// "What changed?" (LLD §9) compares the learner's simulation before and after an edit, so the
// browser keeps the values of the last few circuits it simulated: one record each time the
// simulation settles on a rev. A change is then the newest record before a rev to that rev's,
// named by the changes the learner made in between (the store's `changes`).
import { createStore, type StoreApi } from "zustand/vanilla";
import type { Registry, SimValues } from "../gen/contract.ts";
import type { ChangeMark, CircuitState, CircuitStore } from "../store/circuitStore.ts";
import { simValues } from "./simValues.ts";

/** The simulation values of the circuit at `rev`. */
export interface SimRecord {
  rev: number;
  values: SimValues;
}

/** An edit (or several) between two simulated circuits. */
export interface Change {
  fromRev: number;
  rev: number;
  /** The changes in between, oldest first (labels as the undo history shows them). */
  labels: string[];
  before: SimValues;
  after: SimValues;
}

export interface SimHistoryState {
  /** Oldest first, one per rev, at most `MAX_RECORDS`. */
  records: SimRecord[];
}

export type SimHistory = StoreApi<SimHistoryState> & { dispose(): void };

export const MAX_RECORDS = 10;

const settled = (s: CircuitState) => s.sim.status !== "pending" && s.sim.status !== "running";

/** Record the simulation each time it settles. The simulation goes pending on every rev change and
 * a result for an earlier circuit stays pending (`attachSimulation`), so a settled `sim` always
 * belongs to the rev beside it. */
export function createSimHistory(store: CircuitStore, registry: Registry): SimHistory {
  const history = createStore<SimHistoryState>()(() => ({ records: [] }));
  const record = (s: CircuitState) => {
    if (!settled(s)) return;
    const entry = { rev: s.rev, values: simValues(s, registry) };
    const records = history.getState().records.filter((r) => r.rev !== s.rev);
    history.setState({ records: [...records, entry].slice(-MAX_RECORDS) });
  };
  record(store.getState());
  const unsubscribe = store.subscribe((s, prev) => {
    if (s.sim !== prev.sim) record(s);
  });
  return Object.assign(history, { dispose: unsubscribe });
}

/** The change that ends at `rev` (the newest record by default): from the newest record before it.
 * Null without both simulations. */
export function changeAt(records: SimRecord[], marks: ChangeMark[], rev?: number): Change | null {
  const after = rev === undefined ? records.at(-1) : records.find((r) => r.rev === rev);
  if (!after) return null;
  const before = records.filter((r) => r.rev < after.rev).at(-1);
  if (!before) return null;
  const labels = marks.filter((m) => m.rev > before.rev && m.rev <= after.rev).map((m) => m.label);
  return { fromRev: before.rev, rev: after.rev, labels, before: before.values, after: after.values };
}

/** How a change is named on its button: its one step, or how many there were. */
export function changeName(c: Pick<Change, "labels">): string {
  if (c.labels.length === 1) return c.labels[0]!;
  return c.labels.length ? `your last ${c.labels.length} changes` : "your last change";
}
