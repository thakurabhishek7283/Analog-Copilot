// Text drawn next to symbols and nets. Values come from the core's parsed quantities (`display`),
// never re-parsed here.
import type { PartDef, PartInstance } from "../../gen/contract.ts";

/**
 * The value line under a part's name: its non-zero params ("10kΩ", "1V 1kHz"), or for parts
 * without params the type number from the title ("TL072", "2N3904").
 */
export function partValue(inst: PartInstance, def: PartDef | undefined): string {
  const shown = Object.keys(def?.params ?? inst.params)
    .map((k) => inst.params[k])
    .filter((q) => q !== undefined && q.si !== 0)
    .map((q) => q!.display);
  if (shown.length) return shown.join(" ");
  const first = def?.title.split(/[\s,]/)[0] ?? "";
  return /[A-Za-z]/.test(first) && /\d/.test(first) ? first : "";
}

/** `U1` plus the unit letter for multi-unit parts: `U1A`. */
export function partName(refdes: string, unit: string | null): string {
  return unit ? refdes + unit : refdes;
}

/** Three significant figures, mV below 1 V. Rounded first, so 0.99999 V reads "1 V". */
export function formatVolts(v: number): string {
  const sig3 = (x: number) => x.toPrecision(3).replace(/(\.\d*?)0+$/, "$1").replace(/\.$/, "");
  const r = Number(v.toPrecision(3));
  const a = Math.abs(r);
  if (a < 5e-4) return "0 V";
  return a < 1 ? `${sig3(r * 1e3)} mV` : `${sig3(r)} V`;
}

/** A net's lowest and highest voltage over the transient after start-up. */
export interface Swing {
  min: number;
  max: number;
}

/** A net as its label reads: the swing when a signal moves it ("±705 mV" around zero, "6.63 V
 * ±1 V" on a bias), else the operating point ("12 V"). Dry run: a sine-driven filter read "0 V" on
 * every wire, the DC operating point of a signal centred on zero. */
export function formatNet(op: number | undefined, swing?: Swing): string | undefined {
  if (swing) {
    const amp = (swing.max - swing.min) / 2;
    const mid = (swing.max + swing.min) / 2;
    if (amp >= 1e-3 && amp > 0.01 * Math.abs(mid)) {
      return Math.abs(mid) < Math.max(1e-3, 0.05 * amp) ? `±${formatVolts(amp)}` : `${formatVolts(mid)} ±${formatVolts(amp)}`;
    }
  }
  return op === undefined ? undefined : formatVolts(op);
}

/** What a label means, for its tooltip. */
export function netTitle(op: number | undefined, swing?: Swing): string {
  const parts = [];
  if (swing) parts.push(`swings ${formatVolts(swing.min)} to ${formatVolts(swing.max)} in the transient (after start-up)`);
  if (op !== undefined) parts.push(`${formatVolts(op)} at the operating point (no signal)`);
  return parts.join("; ");
}

const PREFIXES: [number, string][] = [
  [1e9, "G"],
  [1e6, "M"],
  [1e3, "k"],
  [1, ""],
  [1e-3, "m"],
  [1e-6, "µ"],
  [1e-9, "n"],
  [1e-12, "p"],
];

/** Three significant figures with an SI prefix: `formatSi(0.0047, "A")` is "4.7 mA". */
export function formatSi(v: number, unit: string): string {
  if (!Number.isFinite(v)) return "—";
  const a = Math.abs(v);
  if (a < 1e-15) return `0 ${unit}`;
  const [scale, prefix] = PREFIXES.find(([s]) => a >= s * 0.9995) ?? PREFIXES.at(-1)!;
  const x = (v / scale).toPrecision(3).replace(/(\.\d*?)0+$/, "$1").replace(/\.$/, "");
  return `${x} ${prefix}${unit}`;
}
