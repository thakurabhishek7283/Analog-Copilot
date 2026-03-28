// A drag handle between two panels: drag it (or focus it and use the arrow keys) to resize one of
// them; a double click puts that size back to its default.
import { useRef } from "react";
import { LIMITS, type Sizes } from "./panels.ts";

const STEP = 16;

export interface SplitterProps {
  /** `x`: a vertical bar dragged sideways; `y`: a horizontal bar dragged up and down. */
  axis: "x" | "y";
  size: keyof Sizes;
  label: string;
  value: number;
  /** 1: dragging right (or down) makes the panel bigger; -1: smaller. */
  dir: 1 | -1;
  /** The panel's size as drawn now: the CSS can hold it under the stored value. */
  measure: () => number | undefined;
  onResize(px: number): void;
  onReset(): void;
}

export function Splitter({ axis, size, label, value, dir, measure, onResize, onReset }: SplitterProps) {
  const start = useRef<{ pos: number; value: number } | null>(null);
  const pos = (e: React.PointerEvent) => (axis === "x" ? e.clientX : e.clientY);
  const end = (e: React.PointerEvent<HTMLDivElement>) => {
    if (!start.current) return;
    start.current = null;
    e.currentTarget.releasePointerCapture?.(e.pointerId);
    document.documentElement.removeAttribute("data-resizing");
  };
  return (
    <div
      className="splitter"
      role="separator"
      tabIndex={0}
      aria-label={label}
      aria-orientation={axis === "x" ? "vertical" : "horizontal"}
      aria-valuenow={Math.round(measure() ?? value)}
      aria-valuemin={LIMITS[size][0]}
      aria-valuemax={LIMITS[size][1]}
      title={`${label} (double-click to reset)`}
      onPointerDown={(e) => {
        if (e.button !== 0) return;
        e.preventDefault();
        e.currentTarget.setPointerCapture?.(e.pointerId);
        start.current = { pos: pos(e), value: measure() ?? value };
        document.documentElement.setAttribute("data-resizing", axis);
      }}
      onPointerMove={(e) => {
        if (start.current) onResize(start.current.value + dir * (pos(e) - start.current.pos));
      }}
      onPointerUp={end}
      onPointerCancel={end}
      onDoubleClick={onReset}
      onKeyDown={(e) => {
        const keys: Record<string, number> = axis === "x" ? { ArrowLeft: -STEP, ArrowRight: STEP } : { ArrowUp: -STEP, ArrowDown: STEP };
        const step = keys[e.key];
        if (step === undefined) return;
        e.preventDefault();
        onResize((measure() ?? value) + dir * step);
      }}
    />
  );
}
