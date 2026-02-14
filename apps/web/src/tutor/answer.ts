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
}

const shown = (c: CheckResult | undefined) => c?.measured_display ?? "—";

/** The spec checks an experiment moved: measured before, measured after. */
export function changedChecks(before: CheckResult[], after: CheckResult[]): CheckChange[] {
  const key = (c: CheckResult) => `${c.block}\u0000${c.name}`;
  const was = new Map(before.map((c) => [key(c), c]));
  return after
    .filter((c) => shown(was.get(key(c))) !== shown(c))
    .map((c) => ({ block: c.block, name: c.name, label: c.label, before: shown(was.get(key(c))), after: shown(c) }));
}
