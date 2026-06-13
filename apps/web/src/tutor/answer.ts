// What the Ask panel shows of an answer (LLD §9): its body as text and reference chips, an
// experiment's ops in words, and an experiment's checks before and after. Pure functions over the
// core's `Answer`: its reference spans are UTF-16 code units, JavaScript's own string indices.
import type { CheckResult, Op, Ref } from "../gen/contract.ts";

export type Segment = { text: string } | { ref: Ref };

/** The body cut into plain text and references, in order. A reference the circuit does not hold
 * becomes plain text (its id, without brackets): it is never a chip (LLD §9). */
export function segments(body: string, refs: Ref[]): Segment[] {
  const out: Segment[] = [];
  let at = 0;
  const text = (t: string) => {
    if (!t) return;
    const last = out.at(-1);
    if (last && "text" in last) last.text += t;
    else out.push({ text: t });
  };
  for (const r of [...refs].sort((a, b) => a.start - b.start)) {
    if (r.start < at || r.end > body.length) continue; // overlapping or out of range: skip it
    text(body.slice(at, r.start));
    if (r.valid) out.push({ ref: r });
    else text(r.id);
    at = r.end;
  }
  text(body.slice(at));
  return out;
}

/** One op of a suggestion in words: "R1 resistance → 36k". */
export function describeOp(op: Op): string {
  switch (op.op) {
    case "part.set_param":
      return `${op.body.refdes} ${op.body.key} → ${op.body.value}`;
    case "part.swap":
      return `${op.body.refdes} becomes ${op.body.part}`;
    case "part.add": {
      const values = Object.entries(op.body.params ?? {}).map(([k, v]) => `${k} ${v}`);
      return `Add ${op.body.refdes} (${op.body.part}${values.length ? `, ${values.join(", ")}` : ""})`;
    }
    case "part.remove":
      return `Remove ${op.body.refdes}`;
    case "net.connect":
      return `Connect ${op.body.pins.join(", ")} to ${op.body.net}`;
    case "net.disconnect":
      return `Disconnect ${op.body.pins.join(", ")} from ${op.body.net}`;
    case "analysis.set":
      return `Run ${op.body.analyses.map((a) => a.type.toUpperCase()).join(", ") || "no analyses"}`;
    default:
      return op.op;
  }
}

export interface CheckChange {
  block: string;
  name: string;
  label: string;
  before: string;
  after: string;
  /** Whether it meets its target after the change, and the target. */
  pass: boolean;
  target: string;
}

const shown = (c: CheckResult | undefined) => c?.measured_display ?? "—";

/** The spec checks an experiment moved: measured before, measured after. */
export function changedChecks(before: CheckResult[], after: CheckResult[]): CheckChange[] {
  const key = (c: CheckResult) => `${c.block}\u0000${c.name}`;
  const was = new Map(before.map((c) => [key(c), c]));
  return after
    .filter((c) => shown(was.get(key(c))) !== shown(c))
    .map((c) => ({ block: c.block, name: c.name, label: c.label, before: shown(was.get(key(c))), after: shown(c), pass: c.pass, target: c.target_display }));
}

const GENERIC = new Set(["block", "with", "from", "order", "stage"]);

/** The block references to show as a compact chip (by segment index): the model already named the
 * block in the same sentence ("the Sallen-Key filter [block:b2]" repeated the title), or an earlier
 * chip in the answer showed its title. `title` is the block's title, if the circuit holds it. */
export function compactBlocks(segs: Segment[], title: (id: string) => string | undefined): Set<number> {
  const out = new Set<number>();
  const shown = new Set<string>();
  let sentence = "";
  segs.forEach((s, i) => {
    if ("text" in s) {
      const end = Math.max(s.text.lastIndexOf(". "), s.text.lastIndexOf("\n"), s.text.lastIndexOf("? "), s.text.lastIndexOf("! "));
      sentence = end >= 0 ? s.text.slice(end + 1) : sentence + s.text;
      return;
    }
    if (s.ref.kind !== "block" || !s.ref.valid) return;
    const t = title(s.ref.id);
    if (!t) return;
    const words = t.toLowerCase().split(/[^a-z]+/).filter((w) => w.length >= 4 && !GENERIC.has(w));
    const said = sentence.toLowerCase();
    if (shown.has(s.ref.id) || words.some((w) => said.includes(w))) out.add(i);
    else shown.add(s.ref.id);
  });
  return out;
}
