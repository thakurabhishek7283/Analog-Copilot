// An experiment's prediction against what the simulation measured, and the spec checks it retuned on
// purpose (LLD §9: predict, then test). The rule is the tutor evals' (evals/tutor_metrics.py,
// `prediction`); both run the cases in evals/tests/prediction_cases.json.
import type { CheckResult } from "../gen/contract.ts";
import type { AskEntry } from "./tutorStore.ts";

export interface Quantity {
  text: string;
  /** SI, signed. */
  value: number;
  /** Family: ohm, F, H, Hz, V, A, W, s, %, dB, deg, or "" (none). */
  unit: string;
  /** Where it starts in the text. */
  start: number;
}

const PREFIX: Record<string, number> = {
  f: 1e-15, p: 1e-12, n: 1e-9, u: 1e-6, "µ": 1e-6, "μ": 1e-6, m: 1e-3, "": 1, k: 1e3, K: 1e3, M: 1e6, meg: 1e6, G: 1e9,
};
const UNIT: Record<string, string> = {
  "Ω": "ohm", ohm: "ohm", ohms: "ohm", R: "ohm", F: "F", H: "H", Hz: "Hz", V: "V", A: "A", W: "W", s: "s", "%": "%",
  dB: "dB", "°": "deg", deg: "deg", degrees: "deg", "V/V": "",
};

// Python's \w is Unicode; [\p{L}\p{N}_] is the same here.
const QUANTITY = new RegExp(
  String.raw`(?<![\p{L}\p{N}_\]])(?<!\d\.)(?<num>[-−+]?\d+(?:[.,]\d+)?(?:[eE][-+]?\d+)?)` +
    String.raw`(?:\s?(?<pre>meg|[fpnuµμmkKMG])?(?<unit>V\/V|Hz|dB|Ω|ohms?\b|degrees\b|deg\b|°|%|[FHVAWs](?![A-Za-z]))?)` +
    String.raw`(?![\p{L}\p{N}_])`,
  "gu",
);

const LATEX: [RegExp, string][] = [
  [/\\times/g, "×"], [/\\cdot/g, "·"], [/\\approx/g, "≈"], [/\\pi\b/g, "π"], [/\\Omega/g, "Ω"], [/\\mu/g, "µ"],
  [/\\pm/g, "±"], [/\^\{?\\circ\}?/g, "°"], [/\\circ/g, "°"], [/\\%/g, "%"], [/\\parallel/g, "∥"], [/\\[,;:! ]/g, " "],
  [/\\(?:left|right)/g, ""], [/\\[()[\]]/g, " "], [/\$/g, " "],
];

/** LaTeX an answer slipped into (`\(9\times4.9\)`, `\frac{a}{b}`) as the plain text it means. */
export function plain(text: string): string {
  if (!text.includes("\\") && !text.includes("$")) return text;
  let s = text;
  for (let i = 0; i < 3; i++) {
    // nested \frac and \sqrt, innermost first
    s = s.replace(/\\[dt]?frac\{([^{}]*)\}\{([^{}]*)\}/g, "($1)/($2)");
    s = s.replace(/\\sqrt\{([^{}]*)\}/g, "√($1)");
    s = s.replace(/\\(?:text|mathrm|operatorname|mathbf)\{([^{}]*)\}/g, "$1");
  }
  for (const [pattern, repl] of LATEX) s = s.replace(pattern, repl);
  return s.replace(/[{}]/g, "");
}

const number = (raw: string) => Number(raw.replace(/−/g, "-").replace(/,/g, ""));

/** Every quantity in `text`: a number with an optional SI prefix and unit (18kΩ, 0.7 V, 4.7n, -3 dB).
 * Numbers that are part of an identifier (R1, B2_OUT, TL072, 2nd) are skipped. */
export function quantities(text: string): Quantity[] {
  const out: Quantity[] = [];
  for (const m of text.matchAll(QUANTITY)) {
    const { num, pre = "", unit = "" } = m.groups as { num: string; pre?: string; unit?: string };
    if (num.includes(",") && !/^[-−+]?\d{1,3}(,\d{3})+$/.test(num)) continue; // "1,2" is two numbers in a list
    if (pre && !unit && m[0].slice(num.length).startsWith(" ")) continue; // "18 k..." is a word, not a prefix
    out.push({ text: m[0].trim(), value: number(num) * PREFIX[pre]!, unit: UNIT[unit] ?? "", start: m.index });
  }
  return out;
}

// A bound, not a value: "well below 2 kHz" is not a prediction of 2 kHz.
const BOUND = /(?:\b(?:below|above|under|over|less than|more than|at least|at most|up to)|[<>≤≥])\s*$/i;

const CHECK_UNIT: Partial<Record<CheckResult["unit"], string>> = {
  hertz: "Hz", volt: "V", ampere: "A", ohm: "ohm", unitless: "", second: "s",
};

export interface Prediction {
  /** The quantity compared (or the first one in the prediction). */
  number: string | null;
  /** The check it was compared with, "b2 fc_hz". */
  check: string | null;
  measured: number | null;
  /** Null: nothing to compare (no number, or no moved check of its unit). */
  held: boolean | null;
}

type Measured = Pick<CheckResult, "block" | "name" | "unit" | "tol_pct" | "measured">;

/** A check's identity across simulations. */
export const checkKey = (c: Pick<CheckResult, "block" | "name">) => `${c.block}\u0000${c.name}`;

/** Moved by more than 1% (or new). */
function moved(was: Map<string, number | null | undefined>, c: Measured): boolean {
  const b = was.get(checkKey(c));
  return b == null || Math.abs(c.measured! - b) > 0.01 * Math.max(Math.abs(b), 1e-12);
}

/** The `predict` text against the checks measured before and after the experiment (leave out
 * stimulus checks, such as a source's amplitude). Each quantity in it is compared with every check
 * of its unit the experiment moved by more than 1%: it held when one is within 10% or the check's
 * tolerance, whichever is wider; it missed when there were checks to compare and none agreed. A
 * number after a bound ("below", "more than", "<") is not a value to compare. */
export function prediction(predict: string, before: Measured[], after: Measured[]): Prediction {
  const text = plain(predict);
  const qs = quantities(text).filter((q) => !BOUND.test(text.slice(0, q.start)));
  const out: Prediction = { number: qs[0]?.text ?? null, check: null, measured: null, held: null };
  const was = new Map(before.map((c) => [checkKey(c), c.measured]));
  const pool = after.filter((c) => c.measured != null && moved(was, c));
  const err = (q: Quantity, c: Measured) => Math.abs(Math.abs(q.value) - Math.abs(c.measured!)) / Math.max(Math.abs(c.measured!), 1e-12);
  let best: { q: Quantity; c: Measured; e: number } | null = null;
  for (const q of qs) {
    for (const c of pool) {
      if ((CHECK_UNIT[c.unit] ?? "") !== q.unit) continue;
      const e = err(q, c);
      if (!best || e < best.e) best = { q, c, e };
    }
  }
  if (!best) return out;
  const tol = Math.max(0.1, (best.c.tol_pct || 0) / 100);
  return { number: best.q.text, check: `${best.c.block} ${best.c.name}`, measured: best.c.measured!, held: best.e <= tol };
}

/** The checks a measured experiment that is still in place moved and that now miss their target:
 * the learner retuned them on purpose, so they are not failures. A check counts while its value is
 * still the one the experiment measured (within 1%): undoing the experiment, or any later edit that
 * moves the check again, ends it. Keys are `block\0name`. */
export function retunedChecks(entries: AskEntry[], checks: CheckResult[]): Set<string> {
  const out = new Set<string>();
  const now = new Map(checks.map((c) => [checkKey(c), c]));
  for (const e of entries) {
    const t = e.trial;
    if (t?.state !== "measured" || !t.after) continue;
    const was = new Map(t.before.map((c) => [checkKey(c), c.measured]));
    for (const c of t.after) {
      const cur = now.get(checkKey(c));
      if (c.measured == null || !cur || cur.pass || cur.measured == null || !moved(was, c)) continue;
      if (Math.abs(cur.measured - c.measured) <= 0.01 * Math.max(Math.abs(c.measured), 1e-12)) out.add(checkKey(c));
    }
  }
  return out;
}

