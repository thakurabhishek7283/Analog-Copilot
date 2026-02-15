// The tutor's side of the editor (LLD §9): the questions asked in this visit (and the edits it was
// asked to explain, "What changed?") and their answers as they stream, the learner's settings (level, explain or Socratic, how hard a reasoning model
// thinks), feedback, and each tried experiment's prediction next to what the simulation measured.
import { createStore, type StoreApi } from "zustand/vanilla";
import type {
  ApiError,
  AskDone,
  CheckResult,
  LearnerLevel,
  ReasoningEffort,
  Selection,
  TutorMode,
} from "../gen/contract.ts";
import type { Change } from "./simHistory.ts";

/** An experiment from an answer's `try` block, applied as one undo step. */
export interface Trial {
  /** `applied`: waiting for the simulation; `measured`: `after` holds its checks; `undone`. */
  state: "applied" | "measured" | "undone";
  /** The undo step's label, and the rev it left the circuit at (Undo is offered while both hold). */
  label: string;
  rev: number;
  before: CheckResult[];
  after?: CheckResult[];
}

export interface AskEntry {
  key: number;
  /** A question, or "What changed?" about `change`. */
  kind: "ask" | "what_changed";
  question: string;
  selection: Selection | null;
  mode: TutorMode;
  /** The circuit's rev the question was about. */
  rev: number;
  /** The edit a "What changed?" entry explains. */
  change?: Change;
  phase: "asking" | "streaming" | "done" | "failed" | "stopped";
  /** The answer as streamed so far. */
  text: string;
  /** The whole answer, read by the server against the circuit at `rev`. */
  answer?: AskDone["answer"];
  askId?: string;
  error?: ApiError;
  feedback?: 1 | -1;
  trial?: Trial;
}

export interface TutorSettings {
  level: LearnerLevel;
  mode: TutorMode;
  /** Null: the provider's default. */
  effort: ReasoningEffort | null;
}

export interface TutorState {
  entries: AskEntry[];
  settings: TutorSettings;
  setSettings(s: Partial<TutorSettings>): void;
  begin(e: Pick<AskEntry, "kind" | "question" | "selection" | "mode" | "rev" | "change">): number;
  update(key: number, change: Partial<AskEntry> | ((e: AskEntry) => void)): void;
  delta(key: number, text: string): void;
  done(key: number, done: AskDone): void;
  failed(key: number, error: ApiError): void;
  clear(): void;
}

export type TutorStore = StoreApi<TutorState>;

/** At most this many questions are kept on screen. */
export const MAX_ENTRIES = 20;

export function createTutorStore(): TutorStore {
  let next = 0;
  return createStore<TutorState>()((set) => {
    const update: TutorState["update"] = (key, change) =>
      set((s) => ({
        entries: s.entries.map((e) => {
          if (e.key !== key) return e;
          if (typeof change !== "function") return { ...e, ...change };
          const copy = { ...e };
          change(copy);
          return copy;
        }),
      }));
    return {
      entries: [],
      settings: { level: "beginner", mode: "explain", effort: null },
      setSettings: (change) => set((s) => ({ settings: { ...s.settings, ...change } })),
      begin(e) {
        const key = ++next;
        set((s) => ({ entries: [...s.entries, { ...e, key, phase: "asking" as const, text: "" }].slice(-MAX_ENTRIES) }));
        return key;
      },
      update,
      delta: (key, text) =>
        update(key, (e) => {
          e.phase = "streaming";
          e.text += text;
        }),
      done: (key, done) => update(key, { phase: "done", answer: done.answer, askId: done.ask_id }),
      failed: (key, error) => update(key, { phase: "failed", error }),
      clear: () => set({ entries: [] }),
    };
  });
}
