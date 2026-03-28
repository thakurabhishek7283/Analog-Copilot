// The editor's panel layout: column widths, the scope's and side panes' heights, which panes are
// collapsed, and "Focus tutor". It belongs to the browser, not the circuit, so it is kept in
// localStorage (per browser; a private window starts from the defaults) and never synced.
import { createStore, type StoreApi } from "zustand/vanilla";
import { useStore } from "zustand";

export type PaneId = "palette" | "inspector" | "ask" | "lesson";

export interface Sizes {
  /** The palette column's width. */
  palette: number;
  /** The side column's width (inspector, tutor, lesson). */
  side: number;
  /** The scope's plot height. */
  scope: number;
  /** The inspector's and the lesson's heights in the side column; the tutor takes the rest. */
  inspector: number;
  lesson: number;
}

export interface PanelLayout {
  sizes: Sizes;
  collapsed: Record<PaneId, boolean>;
}

export interface PanelState extends PanelLayout {
  /** The layout "Focus tutor" replaced, put back when it is turned off; null when it is off. */
  beforeFocus: PanelLayout | null;
  resize(key: keyof Sizes, px: number): void;
  /** Collapse or expand a pane (toggle without `collapsed`). */
  toggle(id: PaneId, collapsed?: boolean): void;
  /** Give the tutor most of a `viewport`-wide window, or put the earlier layout back. */
  focusTutor(on: boolean, viewport: number): void;
  reset(): void;
}

export type PanelStore = StoreApi<PanelState>;

export const DEFAULT_LAYOUT: PanelLayout = {
  sizes: { palette: 200, side: 400, scope: 220, inspector: 200, lesson: 140 },
  collapsed: { palette: false, inspector: false, ask: false, lesson: false },
};

/** Drag limits; the CSS also keeps the side column from squeezing the schematic below 340 px. */
export const LIMITS: Record<keyof Sizes, readonly [number, number]> = {
  palette: [140, 420],
  side: [280, 1600],
  scope: [80, 1200],
  inspector: [60, 1200],
  lesson: [60, 1200],
};

/** "Focus tutor" widens the side column to this share of the window, at least. */
export const FOCUS_SHARE = 0.45;

const KEY = "analog-copilot.layout";

export const clampSize = (key: keyof Sizes, px: number): number => Math.round(Math.min(LIMITS[key][1], Math.max(LIMITS[key][0], px)));

/** A stored layout read back defensively: anything missing or malformed takes the default. */
function parse(raw: unknown): PanelLayout | null {
  if (!raw || typeof raw !== "object") return null;
  const r = raw as Partial<Record<keyof PanelLayout, Record<string, unknown>>>;
  const sizes = { ...DEFAULT_LAYOUT.sizes };
  for (const k of Object.keys(sizes) as (keyof Sizes)[]) {
    const v = r.sizes?.[k];
    if (typeof v === "number" && Number.isFinite(v)) sizes[k] = clampSize(k, v);
  }
  const collapsed = { ...DEFAULT_LAYOUT.collapsed };
  for (const k of Object.keys(collapsed) as PaneId[]) {
    if (typeof r.collapsed?.[k] === "boolean") collapsed[k] = r.collapsed[k] as boolean;
  }
  return { sizes, collapsed };
}

function load(storage: Storage | null): Pick<PanelState, "sizes" | "collapsed" | "beforeFocus"> {
  try {
    const raw = JSON.parse(storage?.getItem(KEY) ?? "null") as { beforeFocus?: unknown } | null;
    const layout = parse(raw);
    if (layout) return { ...layout, beforeFocus: parse(raw?.beforeFocus) };
  } catch {
    // unreadable: the defaults
  }
  return { ...DEFAULT_LAYOUT, beforeFocus: null };
}

export function isDefaultLayout(s: PanelLayout & { beforeFocus: PanelLayout | null }): boolean {
  return !s.beforeFocus && JSON.stringify({ sizes: s.sizes, collapsed: s.collapsed }) === JSON.stringify(DEFAULT_LAYOUT);
}

export function createPanelStore(storage: Storage | null): PanelStore {
  const store = createStore<PanelState>()((set) => ({
    ...load(storage),
    resize: (key, px) => set((s) => ({ sizes: { ...s.sizes, [key]: clampSize(key, px) } })),
    toggle: (id, collapsed) => set((s) => ({ collapsed: { ...s.collapsed, [id]: collapsed ?? !s.collapsed[id] } })),
    focusTutor: (on, viewport) =>
      set((s) => {
        if (!on) return s.beforeFocus ? { ...s.beforeFocus, beforeFocus: null } : s;
        if (s.beforeFocus) return s;
        return {
          beforeFocus: { sizes: s.sizes, collapsed: s.collapsed },
          sizes: { ...s.sizes, side: clampSize("side", Math.max(s.sizes.side, viewport * FOCUS_SHARE)) },
          collapsed: { palette: true, inspector: true, ask: false, lesson: true },
        };
      }),
    reset: () => set({ ...DEFAULT_LAYOUT, beforeFocus: null }),
  }));
  // Saved shortly after it stops changing (a drag changes it on every pointer move).
  let timer: ReturnType<typeof setTimeout> | undefined;
  store.subscribe((s) => {
    clearTimeout(timer);
    timer = setTimeout(() => {
      try {
        storage?.setItem(KEY, JSON.stringify({ sizes: s.sizes, collapsed: s.collapsed, beforeFocus: s.beforeFocus }));
      } catch {
        // not saved: the next visit starts from the defaults
      }
    }, 150);
  });
  return store;
}

function browserStorage(): Storage | null {
  try {
    return typeof localStorage === "undefined" ? null : localStorage;
  } catch {
    return null;
  }
}

/** The page's layout (one per browser tab, whichever circuit is open). */
export const panels = createPanelStore(browserStorage());

export function usePanels<T>(selector: (s: PanelState) => T): T {
  return useStore(panels, selector);
}
