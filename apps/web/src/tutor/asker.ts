// Asking the tutor about the open project (LLD §9). A question is about the circuit as the server
// holds it, so unsaved edits are sent first; it carries the learner's simulation of that circuit
// (the server never simulated it). "What changed?" sends the simulations before and after an edit
// instead, and the server compares its circuits at both revs. One answer at a time: a new one
// stops the last. An answer's experiment is applied as one undo step, then simulated, and its
// prediction is shown next to what the simulation measured.
import { type ApiClient, ApiFailure } from "../api/client.ts";
import type { AskRequest, ChangeRequest, OpError, Registry, Selection } from "../gen/contract.ts";
import type { CircuitStore } from "../store/circuitStore.ts";
import { askStream } from "./askStream.ts";
import type { Change } from "./simHistory.ts";
import { simSettled, simValues } from "./simValues.ts";
import { type TutorState, type TutorStore, historyIds } from "./tutorStore.ts";

export interface AskerOptions {
  api: Pick<ApiClient, "askUrl" | "whatChangedUrl" | "authHeaders" | "feedback" | "dropToken">;
  project: string;
  store: CircuitStore;
  tutor: TutorStore;
  registry: Registry;
  /** Send unsaved edits: the server answers about the circuit it holds. */
  flush: () => Promise<void>;
  fetch?: typeof fetch;
  /** How long a question or an experiment waits for the simulation to catch up. */
  simWaitMs?: number;
}

export interface Asker {
  ask(question: string, selection: Selection | null): Promise<void>;
  /** Explain an edit from the simulations before and after it ("What changed?"). */
  whatChanged(change: Change): Promise<void>;
  /** Stop the answer being written (what arrived stays). */
  stop(): void;
  feedback(key: number, value: 1 | -1): Promise<void>;
  /** Apply an answer's experiment as one undo step; the core's refusal, if any. */
  tryIt(key: number): Promise<OpError | null>;
  /** Undo an experiment, while it is still the last change. */
  undoTrial(key: number): boolean;
  dispose(): void;
}

const MAX_LABEL = 60;

export function createAsker(opts: AskerOptions): Asker {
  const { api, store, tutor } = opts;
  const simWaitMs = opts.simWaitMs ?? 5000;
  let current: AbortController | null = null;
  const entry = (key: number) => tutor.getState().entries.find((e) => e.key === key);

  /** One answer into a new entry: unsaved edits sent first, then the request `body` builds (once
   * the server holds the circuit) streamed into it. */
  const answer = async (
    init: Parameters<TutorState["begin"]>[0],
    url: string,
    body: (key: number) => Promise<AskRequest | ChangeRequest>,
  ) => {
    current?.abort();
    const controller = new AbortController();
    current = controller;
    const key = tutor.getState().begin(init);
    try {
      await opts.flush();
      const request = await body(key);
      if (controller.signal.aborted) throw controller.signal.reason;
      await askStream({
        url,
        headers: () => api.authHeaders(),
        body: request,
        signal: controller.signal,
        fetch: opts.fetch,
        onEvent: (e) => {
          if (e.event === "answer.delta") tutor.getState().delta(key, e.data.text);
          else if (e.event === "answer.done") tutor.getState().done(key, e.data);
          else tutor.getState().failed(key, e.data);
        },
      });
    } catch (e) {
      if (controller.signal.aborted) {
        tutor.getState().update(key, (x) => {
          if (x.phase === "asking" || x.phase === "streaming") x.phase = "stopped";
        });
        return;
      }
      if (e instanceof ApiFailure && e.status === 401) api.dropToken();
      tutor.getState().failed(key, e instanceof ApiFailure ? e.error : { code: "internal", message: String(e) });
    } finally {
      if (current === controller) current = null;
    }
  };

  const ask = (question: string, selection: Selection | null) => {
    const { level, mode, effort } = tutor.getState().settings;
    const init = { kind: "ask" as const, question, selection, mode, rev: store.getState().rev };
    return answer(init, api.askUrl(opts.project), async (key) => {
      await simSettled(store, simWaitMs);
      const state = store.getState();
      const busy = state.sim.status === "pending" || state.sim.status === "running";
      tutor.getState().update(key, { rev: state.rev });
      const history = historyIds(tutor.getState(), key);
      return {
        question,
        rev: state.rev,
        level,
        mode,
        // Values from an earlier circuit would mislead: none rather than stale ones.
        sim: busy ? {} : simValues(state, opts.registry),
        ...(selection ? { selection } : {}),
        ...(effort ? { effort } : {}),
        ...(history.length ? { history } : {}),
      } satisfies AskRequest;
    });
  };

  const whatChanged = (change: Change) => {
    const { level, mode, effort } = tutor.getState().settings;
    const init = { kind: "what_changed" as const, question: "What changed?", selection: null, mode, rev: change.rev, change };
    return answer(init, api.whatChangedUrl(opts.project), async () => ({
      from_rev: change.fromRev,
      rev: change.rev,
      before: change.before,
      after: change.after,
      level,
      mode,
      ...(effort ? { effort } : {}),
    } satisfies ChangeRequest));
  };

  return {
    ask,
    whatChanged,
    stop: () => current?.abort(),

    async feedback(key, value) {
      const e = entry(key);
      if (!e?.askId) return;
      const before = e.feedback;
      tutor.getState().update(key, { feedback: value });
      try {
        await api.feedback(e.askId, value);
      } catch (err) {
        console.warn("feedback not saved", err);
        tutor.getState().update(key, { feedback: before });
      }
    },

    async tryIt(key) {
      const suggestion = entry(key)?.answer?.try;
      if (!suggestion || suggestion.problems.length) return null;
      const before = store.getState().sim.checks ?? [];
      const predict = suggestion.predict.trim();
      const label = predict ? `Try: ${predict.length > MAX_LABEL ? `${predict.slice(0, MAX_LABEL - 1)}…` : predict}` : "Try the tutor's experiment";
      const r = store.getState().applyBatch(suggestion.ops, label);
      if (r.err) return r.err;
      const rev = r.ok.rev;
      tutor.getState().update(key, { trial: { state: "applied", label, rev, before } });
      await simSettled(store, simWaitMs * 2);
      const s = store.getState();
      if (s.rev !== rev) return null; // edited again before it was measured: nothing to compare
      tutor.getState().update(key, (x) => {
        if (x.trial?.rev === rev && x.trial.state === "applied") x.trial = { ...x.trial, state: "measured", after: s.sim.checks ?? [] };
      });
      return null;
    },

    undoTrial(key) {
      const trial = entry(key)?.trial;
      const s = store.getState();
      if (!trial || trial.state === "undone" || s.rev !== trial.rev || s.history.undo.at(-1)?.label !== trial.label) return false;
      if (!s.undo()) return false;
      tutor.getState().update(key, (x) => {
        if (x.trial) x.trial = { ...x.trial, state: "undone" };
      });
      return true;
    },

    dispose() {
      current?.abort();
    },
  };
}
