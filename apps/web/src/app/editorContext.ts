import { createContext, useContext, useMemo } from "react";
import { useStore } from "zustand";
import type { CircuitState } from "../store/circuitStore.ts";
import type { UiState } from "../store/uiStore.ts";
import type { GenerationState } from "../store/generationStore.ts";
import { retunedChecks } from "../tutor/predict.ts";
import { tranRange } from "../tutor/simValues.ts";
import type { Swing } from "../views/schematic/labels.ts";
import type { TutorState } from "../tutor/tutorStore.ts";
import type { Editor } from "./editor.ts";
import type { ProjectSession, ProjectState } from "./project.ts";

export type EditorContextValue = Omit<Editor, "dispose"> & {
  /** The server project this circuit is, or null (the demo, or the server cannot be reached). */
  project?: ProjectSession | null;
};

export const EditorContext = createContext<EditorContextValue | null>(null);

export function useEditor(): EditorContextValue {
  const ctx = useContext(EditorContext);
  if (!ctx) throw new Error("useEditor outside <EditorContext>");
  return ctx;
}

export function useCircuit<T>(selector: (s: CircuitState) => T): T {
  return useStore(useEditor().store, selector);
}

export function useUi<T>(selector: (s: UiState) => T): T {
  return useStore(useEditor().ui, selector);
}

export function useGen<T>(selector: (s: GenerationState) => T): T {
  return useStore(useEditor().gen, selector);
}

export function useTutor<T>(selector: (s: TutorState) => T): T {
  return useStore(useEditor().tutor, selector);
}

/** The spec checks the learner's experiments retuned on purpose (`checkKey`s): shown as retuned, not failed. */
export function useRetuned(): Set<string> {
  const entries = useTutor((s) => s.entries);
  const checks = useCircuit((s) => s.sim.checks);
  return useMemo(() => retunedChecks(entries, checks ?? []), [entries, checks]);
}

const NO_SWINGS: Record<string, Swing | undefined> = {};

/** Each net's swing in the latest transient (after start-up), for labels that show a signal. */
export function useSwings(): Record<string, Swing | undefined> {
  const tran = useCircuit((s) => s.sim.view?.tran);
  return useMemo(() => tranRange(tran ?? null)?.v ?? NO_SWINGS, [tran]);
}

const NO_PROJECT = { getState: () => null, subscribe: () => () => {} };

/** The open project's state (sync status, title), or null for a local circuit. */
export function useProject<T>(selector: (s: ProjectState) => T): T | null {
  const project = useEditor().project;
  return useStore((project?.state ?? NO_PROJECT) as never, (s: ProjectState | null) => (s ? selector(s) : null));
}
