// The editor's panels, laid out by the learner: drag the borders between the palette, the
// schematic, the scope and the side column; collapse any pane to its header; "Focus" gives the
// tutor most of the window. The layout is per browser (panels.ts).
import { type CSSProperties, type ReactNode, lazy, Suspense, useEffect, useRef, useState } from "react";
import { Schematic } from "../views/schematic/Schematic.tsx";
import { Scope } from "../views/scope/Scope.tsx";
import { AskPanel } from "./AskPanel.tsx";
import { useUi } from "./editorContext.ts";
import { GeneratePanel } from "./GeneratePanel.tsx";
import { Inspector } from "./Inspector.tsx";
import { LessonPanel, useLessonLines } from "./LessonPanel.tsx";
import { Palette } from "./Palette.tsx";
import { DEFAULT_LAYOUT, type PaneId, type Sizes, panels, usePanels } from "./panels.ts";
import { Splitter } from "./Splitter.tsx";

/** The collapsed palette's width. */
const PALETTE_STRIP = 34;
const Breadboard = lazy(() => import("../views/breadboard/Breadboard.tsx").then((module) => ({ default: module.Breadboard })));

export function Workspace() {
  const [view, setView] = useState<"schematic" | "breadboard">("schematic");
  const [breadboardOpened, setBreadboardOpened] = useState(false);
  const sizes = usePanels((s) => s.sizes);
  const paletteHidden = usePanels((s) => s.collapsed.palette);
  const scopeOpen = useUi((s) => s.scope.open);
  const main = useRef<HTMLElement>(null);
  const measure = (selector: string, axis: "x" | "y") => () => {
    const r = main.current?.querySelector(selector)?.getBoundingClientRect();
    return r && (axis === "x" ? r.width : r.height);
  };
  const splitter = (size: keyof Sizes, axis: "x" | "y", dir: 1 | -1, label: string, selector: string) => (
    <Splitter
      axis={axis}
      size={size}
      label={label}
      value={sizes[size]}
      dir={dir}
      measure={measure(selector, axis)}
      onResize={(px) => panels.getState().resize(size, px)}
      onReset={() => panels.getState().resize(size, DEFAULT_LAYOUT.sizes[size])}
    />
  );
  const vars = {
    "--palette-w": `${paletteHidden ? PALETTE_STRIP : sizes.palette}px`,
    "--side-w": `${sizes.side}px`,
    "--scope-h": `${sizes.scope}px`,
  } as CSSProperties;

  return (
    <main ref={main} style={vars}>
      {paletteHidden ? (
        <nav className="palette collapsed" aria-label="Tools and parts">
          <button type="button" aria-label="Show tools and parts" title="Show tools and parts" onClick={() => panels.getState().toggle("palette", false)}>
            »
          </button>
        </nav>
      ) : (
        <Palette />
      )}
      {!paletteHidden && <div className="col-split left">{splitter("palette", "x", 1, "Resize the parts column", ".palette")}</div>}
      <div className="center">
        <GeneratePanel />
        <div className="circuit-view-tabs" role="tablist" aria-label="Circuit view">
          <button type="button" role="tab" aria-selected={view === "schematic"} onClick={() => setView("schematic")}>Schematic</button>
          <button type="button" role="tab" aria-selected={view === "breadboard"} onClick={() => { setBreadboardOpened(true); setView("breadboard"); }}>3D breadboard</button>
        </div>
        <div className="workspace-view" role="tabpanel" aria-label={view === "schematic" ? "Schematic" : "3D breadboard"}>
          <div className="workspace-layer" hidden={view !== "schematic"}><Schematic /></div>
          {breadboardOpened && <div className="workspace-layer" hidden={view !== "breadboard"}><Suspense fallback={<div className="splash">Loading 3D breadboard…</div>}><Breadboard /></Suspense></div>}
        </div>
        {scopeOpen && splitter("scope", "y", -1, "Resize the scope", ".scope-body")}
        <Scope />
      </div>
      <div className="col-split right">{splitter("side", "x", -1, "Resize the side column", ".side")}</div>
      <SidePanes />
    </main>
  );
}

const TITLES: Record<Exclude<PaneId, "palette">, string> = { inspector: "Inspector", ask: "Ask the tutor", lesson: "Lesson" };
/** The pane that takes the column's spare height: the tutor while it is open. */
const FILL_ORDER = ["ask", "lesson", "inspector"] as const;
/** The inspector's height while it holds an Insert block form (dry run: at 200 px the Insert button
 * was below the fold). The learner's own size comes back when the form closes. */
export const INSERT_PANE = 460;

function SidePanes() {
  const sizes = usePanels((s) => s.sizes);
  const collapsed = usePanels((s) => s.collapsed);
  const focused = usePanels((s) => !!s.beforeFocus);
  const hasLesson = useLessonLines().length > 0;
  const inserting = useUi((s) => s.inserting);
  const side = useRef<HTMLDivElement>(null);

  // Insert block opens its form in the inspector, so a collapsed inspector opens for it.
  useEffect(() => {
    if (inserting) panels.getState().toggle("inspector", false);
  }, [inserting]);

  const shown = (["inspector", "ask", "lesson"] as const).filter((id) => id !== "lesson" || hasLesson);
  const fill = FILL_ORDER.find((id) => shown.includes(id) && !collapsed[id]) ?? null;
  const height = (id: "inspector" | "lesson") => () => side.current?.querySelector(`[data-pane="${id}"]`)?.getBoundingClientRect().height;
  const shownSize = (id: "inspector" | "lesson") => (id === "inspector" && inserting ? Math.max(sizes.inspector, INSERT_PANE) : sizes[id]);

  const content: Record<(typeof shown)[number], ReactNode> = { inspector: <Inspector />, ask: <AskPanel />, lesson: <LessonPanel /> };
  const out: ReactNode[] = [];
  shown.forEach((id, i) => {
    const above = shown[i - 1];
    // A handle between two open panes resizes the one that does not fill.
    if (above && !collapsed[above] && !collapsed[id]) {
      const [size, dir] = above !== fill ? ([above, 1] as const) : ([id, -1] as const);
      if (size !== "ask") {
        out.push(
          <Splitter
            key={`split-${id}`}
            axis="y"
            size={size}
            label={`Resize the ${TITLES[size].toLowerCase()}`}
            value={shownSize(size)}
            dir={dir}
            measure={height(size)}
            onResize={(px) => panels.getState().resize(size, px)}
            onReset={() => panels.getState().resize(size, DEFAULT_LAYOUT.sizes[size])}
          />,
        );
      }
    }
    out.push(
      <Pane
        key={id}
        id={id}
        title={TITLES[id]}
        fill={fill === id}
        height={id === "ask" ? 0 : shownSize(id)}
        actions={
          id === "ask" && (
            <button
              type="button"
              className="pane-action"
              aria-pressed={focused}
              title={focused ? "Put the other panels back" : "Give the tutor most of the window"}
              onClick={() => panels.getState().focusTutor(!focused, window.innerWidth)}
            >
              {focused ? "Exit focus" : "Focus"}
            </button>
          )
        }
      >
        {content[id]}
      </Pane>,
    );
  });
  return (
    <div className="side" ref={side}>
      {out}
    </div>
  );
}

function Pane({ id, title, fill, height, actions, children }: { id: PaneId; title: string; fill: boolean; height: number; actions?: ReactNode; children: ReactNode }) {
  const collapsed = usePanels((s) => s.collapsed[id]);
  return (
    <div
      className={collapsed ? "pane collapsed" : fill ? "pane fill" : "pane sized"}
      data-pane={id}
      style={collapsed || fill ? undefined : { flexBasis: `${height}px` }}
    >
      <header className="pane-head">
        <button type="button" className="pane-toggle" aria-expanded={!collapsed} aria-controls={`pane-${id}`} onClick={() => panels.getState().toggle(id)}>
          <span className="chevron" aria-hidden="true">
            {collapsed ? "▸" : "▾"}
          </span>
          {title}
        </button>
        {actions}
      </header>
      {/* Kept mounted while collapsed, so a half-written question or insert form survives. */}
      <div className="pane-body" id={`pane-${id}`} hidden={collapsed}>
        {children}
      </div>
    </div>
  );
}
