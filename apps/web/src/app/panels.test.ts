import { afterEach, describe, expect, it, vi } from "vitest";
import { DEFAULT_LAYOUT, LIMITS, createPanelStore, isDefaultLayout } from "./panels.ts";

function memoryStorage(initial?: unknown): Storage & { data: Map<string, string> } {
  const data = new Map<string, string>(initial === undefined ? [] : [["analog-copilot.layout", typeof initial === "string" ? initial : JSON.stringify(initial)]]);
  return {
    data,
    get length() {
      return data.size;
    },
    clear: () => data.clear(),
    getItem: (k) => data.get(k) ?? null,
    key: (i) => [...data.keys()][i] ?? null,
    removeItem: (k) => void data.delete(k),
    setItem: (k, v) => void data.set(k, v),
  };
}

afterEach(() => vi.useRealTimers());

describe("panel layout", () => {
  it("starts from the defaults, and sizes stay inside their limits", () => {
    const s = createPanelStore(memoryStorage());
    expect(s.getState().sizes).toEqual(DEFAULT_LAYOUT.sizes);
    expect(isDefaultLayout(s.getState())).toBe(true);
    s.getState().resize("side", 10_000);
    expect(s.getState().sizes.side).toBe(LIMITS.side[1]);
    s.getState().resize("palette", 3);
    expect(s.getState().sizes.palette).toBe(LIMITS.palette[0]);
    s.getState().resize("scope", 333.6);
    expect(s.getState().sizes.scope).toBe(334);
    expect(isDefaultLayout(s.getState())).toBe(false);
  });

  it("collapses and expands panes", () => {
    const s = createPanelStore(null);
    s.getState().toggle("lesson");
    expect(s.getState().collapsed.lesson).toBe(true);
    s.getState().toggle("lesson");
    expect(s.getState().collapsed.lesson).toBe(false);
    s.getState().toggle("inspector", false);
    expect(s.getState().collapsed.inspector).toBe(false);
  });

  it("Focus gives the tutor most of the window, and turning it off puts the layout back", () => {
    const s = createPanelStore(null);
    s.getState().resize("inspector", 300);
    s.getState().toggle("palette", false);
    const before = { sizes: s.getState().sizes, collapsed: s.getState().collapsed };
    s.getState().focusTutor(true, 2000);
    expect(s.getState().sizes.side).toBe(900);
    expect(s.getState().collapsed).toEqual({ palette: true, inspector: true, ask: false, lesson: true });
    s.getState().focusTutor(true, 2000); // already on: unchanged
    s.getState().toggle("inspector", false); // the learner opens one while focused
    s.getState().focusTutor(false, 2000);
    expect({ sizes: s.getState().sizes, collapsed: s.getState().collapsed }).toEqual(before);
    expect(s.getState().beforeFocus).toBeNull();
  });

  it("Focus never narrows a side column already wider", () => {
    const s = createPanelStore(null);
    s.getState().resize("side", 1000);
    s.getState().focusTutor(true, 1366);
    expect(s.getState().sizes.side).toBe(1000);
  });

  it("is saved once it stops changing, and read back on the next visit", () => {
    vi.useFakeTimers();
    const storage = memoryStorage();
    const s = createPanelStore(storage);
    for (let px = 300; px <= 500; px += 10) s.getState().resize("side", px);
    expect(storage.data.size).toBe(0);
    vi.advanceTimersByTime(200);
    s.getState().focusTutor(true, 1600);
    vi.advanceTimersByTime(200);
    const again = createPanelStore(storage);
    expect(again.getState().sizes.side).toBe(720);
    expect(again.getState().collapsed.inspector).toBe(true);
    expect(again.getState().beforeFocus?.sizes.side).toBe(500);
    again.getState().reset();
    expect(isDefaultLayout(again.getState())).toBe(true);
  });

  it("reads a damaged or partial stored layout as the defaults where it is wrong", () => {
    expect(createPanelStore(memoryStorage("{not json")).getState().sizes).toEqual(DEFAULT_LAYOUT.sizes);
    const partial = createPanelStore(memoryStorage({ sizes: { side: 520, scope: "big", palette: -5 }, collapsed: { lesson: true, ask: "no" } })).getState();
    expect(partial.sizes).toEqual({ ...DEFAULT_LAYOUT.sizes, side: 520, palette: LIMITS.palette[0] });
    expect(partial.collapsed).toEqual({ ...DEFAULT_LAYOUT.collapsed, lesson: true });
    expect(partial.beforeFocus).toBeNull();
  });

  it("works without storage (a private window that throws)", () => {
    const throwing = { getItem: () => { throw new Error("denied"); }, setItem: () => { throw new Error("denied"); } } as unknown as Storage;
    vi.useFakeTimers();
    const s = createPanelStore(throwing);
    s.getState().resize("side", 400);
    expect(() => vi.advanceTimersByTime(200)).not.toThrow();
    expect(s.getState().sizes.side).toBe(400);
  });
});
