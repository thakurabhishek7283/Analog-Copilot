import { useCallback, useEffect, useMemo, useState } from "react";
import { useCircuit, useEditor, useGen } from "../../app/editorContext.ts";
import { Board3D } from "./Board3D.tsx";
import { BOARD_ROWS, strip, type BreadboardLayout, type Jumper } from "./model.ts";
import "./breadboard.css";

type Pick = { kind: "hole" | "part" | "wire"; id: string };
type Mode = "select" | "wire" | "move-part" | "move-from" | "move-to";
type Status = "unchecked" | "generating" | "checking" | "valid" | "invalid";

function cached(key: string, rev: number): BreadboardLayout | null {
  try {
    const value = JSON.parse(localStorage.getItem(key) ?? "null") as BreadboardLayout | null;
    return value?.rev === rev && Number.isInteger(value.boards) && value.boards >= 1 && value.boards <= 8 && value.parts && Array.isArray(value.jumpers) ? value : null;
  } catch { return null; }
}

export function Breadboard() {
  const editor = useEditor();
  const rev = useCircuit((s) => s.rev);
  const mode = useCircuit((s) => s.mode);
  const parts = useCircuit((s) => s.parts);
  const verification = useGen((s) => s.verification);
  const finalized = !!editor.project && mode !== "generating" && verification?.status === "passed" && verification.rev === rev;
  const key = `analog-copilot.build-guide.${editor.project?.id ?? "local"}.${rev}`;
  const partTypes = useMemo(() => Object.fromEntries(Object.values(parts).map((part) => [part.refdes, part.part])), [parts]);
  const [layout, setLayout] = useState<BreadboardLayout | null>(() => cached(key, rev));
  const [status, setStatus] = useState<Status>("unchecked");
  const [error, setError] = useState<string | null>(null);
  const [notes, setNotes] = useState<string[]>([]);
  const [selected, setSelected] = useState<Pick | null>(null);
  const [fitSignal, setFitSignal] = useState(0);
  const [action, setAction] = useState<Mode>("select");
  const [firstHole, setFirstHole] = useState<string | null>(null);

  useEffect(() => {
    setLayout(cached(key, rev));
    setStatus("unchecked");
    setSelected(null);
    setAction("select");
    setError(null);
  }, [key, rev]);
  useEffect(() => {
    if (!layout || !finalized) return;
    try { localStorage.setItem(key, JSON.stringify(layout)); } catch { /* storage may be unavailable */ }
  }, [key, layout, finalized]);

  const generate = async () => {
    if (!editor.project || !finalized || status === "generating" || status === "checking") return;
    setStatus("generating");
    setError(null);
    try {
      const result = await editor.project.generateBreadboard(rev);
      if (editor.store.getState().rev !== rev) throw new Error("The 2D circuit changed while creating this guide.");
      setLayout(result.layout);
      setNotes(result.notes);
      setStatus("valid");
      setSelected(null);
      setAction("select");
    } catch (e) { setStatus("invalid"); setError(e instanceof Error ? e.message : String(e)); }
  };

  const check = async () => {
    if (!editor.project || !layout || !finalized || status === "generating" || status === "checking") return;
    setStatus("checking");
    setError(null);
    try {
      const result = await editor.project.validateBreadboard(layout);
      setStatus(result.valid ? "valid" : "invalid");
      setError(result.problems.join(" ") || null);
    } catch (e) { setStatus("invalid"); setError(e instanceof Error ? e.message : String(e)); }
  };

  const commit = useCallback((next: BreadboardLayout) => {
    setLayout(next);
    setStatus("unchecked");
    setError(null);
  }, []);

  const setJumper = useCallback((jumper: Jumper) => {
    if (!layout) return;
    commit({ ...layout, jumpers: layout.jumpers.map((wire) => wire.id === jumper.id ? jumper : wire) });
    setAction("select");
  }, [layout, commit]);

  const pick = useCallback((item: Pick) => {
    if (item.kind !== "hole") { setSelected(item); setAction("select"); setFirstHole(null); return; }
    if (action === "select") setSelected(item);
    if (!layout || !finalized) return;
    if (action === "wire") {
      if (!firstHole) { setFirstHole(item.id); return; }
      if (strip(firstHole) === strip(item.id)) { setError("Choose holes on different tie strips."); return; }
      const number = Math.max(0, ...layout.jumpers.map((wire) => Number(wire.id.slice(1)) || 0)) + 1;
      const jumper = { id: `J${number}`, from: firstHole, to: item.id };
      commit({ ...layout, jumpers: [...layout.jumpers, jumper] });
      setAction("select"); setFirstHole(null); setSelected({ kind: "wire", id: jumper.id });
      return;
    }
    if (action === "move-from" || action === "move-to") {
      const wire = layout.jumpers.find((value) => value.id === selected?.id);
      if (wire) setJumper({ ...wire, [action === "move-from" ? "from" : "to"]: item.id });
      return;
    }
    if (action === "move-part" && selected?.kind === "part") {
      const placed = layout.parts[selected.id];
      const target = /^B(\d+):[A-J](\d+)$/.exec(item.id);
      if (!placed?.board || !target) return;
      const board = Number(target[1]);
      const row = Number(target[2]);
      const holes = Object.fromEntries(Object.entries(placed.holes).map(([pin, hole]) => {
        const match = /^B\d+:([A-J])(\d+)$/.exec(hole)!;
        return [pin, `B${board}:${match[1]}${Number(match[2]) + row - placed.row}`];
      }));
      if (Object.values(holes).some((hole) => {
        const match = /^B\d+:[A-J](\d+)$/.exec(hole);
        return !match || Number(match[1]) < 1 || Number(match[1]) > BOARD_ROWS;
      })) { setError("That package does not fit on the board."); return; }
      commit({ ...layout, parts: { ...layout.parts, [selected.id]: { ...placed, board, row, holes } } });
      setAction("select");
    }
  }, [layout, finalized, action, firstHole, commit, selected, setJumper]);

  const removeWire = () => {
    if (!layout || selected?.kind !== "wire") return;
    commit({ ...layout, jumpers: layout.jumpers.filter((wire) => wire.id !== selected.id) });
    setSelected(null);
  };
  const terminals = layout ? Object.values(layout.parts).flatMap((part) => Object.values(part.holes).filter((hole) => hole.startsWith("X:"))) : [];
  const usable = !!layout && finalized && status !== "checking" && status !== "generating";

  return <section className="breadboard" aria-label="3D breadboard">
    <div className="breadboard-toolbar">
      <strong>Physical build guide</strong>
      <button type="button" disabled={!finalized || status === "generating" || status === "checking"} onClick={() => void generate()}>{status === "generating" ? "Generating…" : layout ? "Regenerate with AI" : "Generate build guide"}</button>
      <button type="button" disabled={!usable} onClick={() => void check()}>{status === "checking" ? "Checking…" : "Check guide"}</button>
      <button type="button" aria-pressed={action === "wire"} disabled={!usable} onClick={() => { setAction("wire"); setFirstHole(null); }}>Add jumper</button>
      <button type="button" aria-pressed={action === "move-part"} disabled={!usable || selected?.kind !== "part" || !layout.parts[selected.id]?.board} onClick={() => setAction("move-part")}>Move part</button>
      <button type="button" aria-pressed={action === "move-from"} disabled={!usable || selected?.kind !== "wire"} onClick={() => setAction("move-from")}>Move wire start</button>
      <button type="button" aria-pressed={action === "move-to"} disabled={!usable || selected?.kind !== "wire"} onClick={() => setAction("move-to")}>Move wire end</button>
      <button type="button" disabled={!usable || selected?.kind !== "wire"} onClick={removeWire}>Remove wire</button>
      <button type="button" disabled={!usable || (layout?.boards ?? 8) >= 8} onClick={() => { if (layout) commit({ ...layout, boards: layout.boards + 1 }); }}>Add board</button>
      <button type="button" disabled={!layout || !finalized} onClick={() => setFitSignal((value) => value + 1)}>Fit boards</button>
      <span className="breadboard-status">{!finalized ? "Requires a finalized, passed 2D verification" : status === "valid" ? "✓ Wiring matches 2D" : status === "unchecked" && layout ? "Guide needs checking" : status === "invalid" ? "Guide invalid" : status === "generating" ? "Planning placement…" : status === "checking" ? "Checking…" : "Ready to generate"}</span>
    </div>
    <div className="breadboard-main">
      {layout && finalized ? <Board3D layout={layout} partTypes={partTypes} selected={selected} fitSignal={fitSignal} onPick={pick} /> : <div className="breadboard-empty">{!finalized ? "Finish and verify the 2D circuit first. This optional guide will use that exact saved revision." : error ?? "Generate a physical build guide from the verified circuit."}</div>}
      <aside className="breadboard-details">
        <p>{action === "wire" ? firstHole ? `Select the other end from ${firstHole}.` : "Select the first jumper hole." : action === "move-part" ? `Select a board hole at the new starting row for ${selected?.id}.` : action === "move-from" || action === "move-to" ? `Select the new ${action === "move-from" ? "start" : "end"} hole for ${selected?.id}.` : "Drag to rotate, scroll to zoom. Select a part or jumper to edit it."}</p>
        {error && <p className="breadboard-error" role="alert">{error}</p>}
        {selected && <p>Selected: {selected.kind} {selected.id}</p>}
        {notes.map((note) => <p className="breadboard-note" key={note}>{note}</p>)}
        {terminals.length > 0 && <><h3>External terminals</h3><div className="breadboard-items">{terminals.map((hole) => <button type="button" key={hole} onClick={() => pick({ kind: "hole", id: hole })}>{hole}</button>)}</div></>}
        {layout && <><h3>Parts</h3><div className="breadboard-items">{Object.entries(layout.parts).map(([ref, placed]) => <button type="button" key={ref} aria-pressed={selected?.kind === "part" && selected.id === ref} onClick={() => setSelected({ kind: "part", id: ref })}>{ref} · {placed.board ? `board ${placed.board}, row ${placed.row}` : "external"}<small>{Object.entries(parts[ref]?.params ?? {}).map(([name, value]) => `${name}: ${value?.display ?? "?"}`).join(", ")}</small><small>{placed.package} · {Object.entries(placed.holes).map(([pin, hole]) => `${pin}: ${hole}`).join(", ")}</small></button>)}</div></>}
        {layout && <><h3>Jumpers ({layout.jumpers.length})</h3><div className="breadboard-items">{layout.jumpers.map((wire) => <button type="button" key={wire.id} aria-pressed={selected?.kind === "wire" && selected.id === wire.id} onClick={() => setSelected({ kind: "wire", id: wire.id })}>{wire.id}: {wire.from} → {wire.to}</button>)}</div></>}
        <p className="breadboard-note">3D edits only change this build guide. Use Check guide after edits; the verified 2D circuit stays fixed. Wiring is checked against the schematic, while physical fit depends on buying the listed package and confirming its pinout. The guide is saved in this browser for this circuit revision.</p>
      </aside>
    </div>
  </section>;
}
