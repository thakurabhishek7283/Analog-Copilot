// A Try-it's prediction against the simulation, by the tutor evals' own cases, and the checks it
// retuned on purpose.
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";
import type { CheckResult } from "../gen/contract.ts";
import { plain, prediction, quantities, retunedChecks } from "./predict.ts";
import type { AskEntry, Trial } from "./tutorStore.ts";

const repo = join(import.meta.dirname, "../../../..");

interface Spec {
  checks: Record<string, Pick<CheckResult, "block" | "name" | "unit" | "tol_pct">>;
  cases: { why: string; predict: string; before: Record<string, number | null>; after: Record<string, number | null>; expect: Record<string, unknown> }[];
}

describe("prediction", () => {
  // One rule, two readings: evals/tests/test_tutor_metrics.py runs the same cases.
  const spec = JSON.parse(readFileSync(join(repo, "evals/tests/prediction_cases.json"), "utf8")) as Spec;
  const checks = (values: Record<string, number | null>) => Object.entries(values).map(([k, measured]) => ({ ...spec.checks[k]!, measured }));
  for (const c of spec.cases) {
    it(c.why, () => {
      const got = prediction(c.predict, checks(c.before), checks(c.after));
      expect(Object.fromEntries(Object.keys(c.expect).map((k) => [k, got[k as keyof typeof got]]))).toEqual(c.expect);
    });
  }

  it("reads quantities as the evals do", () => {
    expect(quantities("18kΩ, 10 nF, −3 dB and 1,000 Hz").map((q) => [q.text, q.unit])).toEqual([
      ["18kΩ", "ohm"], ["10 nF", "F"], ["−3 dB", "dB"], ["1,000 Hz", "Hz"],
    ]);
    expect(quantities("R1, B2_OUT, TL072, a 2nd-order filter")).toEqual([]);
    expect(quantities("10 meters").map((q) => q.text)).toEqual(["10"]);
    expect(plain(String.raw`\(\frac{1}{2\pi RC}\) \approx 1\,\text{kHz}`)).toBe(" (1)/(2π RC)  ≈ 1 kHz");
  });
});

const check = (measured: number, pass: boolean): CheckResult => ({
  block: "b2", name: "fc_hz", label: "Cutoff", symbol: "fc", unit: "hertz", target: 1000, tol_pct: 10, measured, pass,
  target_display: "1kHz", measured_display: `${measured}Hz`,
});
const entry = (trial: Trial): AskEntry => ({
  key: 1, kind: "ask", question: "q", selection: null, mode: "explain", rev: 3, phase: "done", text: "", trial,
});

describe("retunedChecks", () => {
  const measured: Trial = { state: "measured", label: "Try: 500 Hz", rev: 4, before: [check(996, true)], after: [check(499, false)] };

  it("marks a check the experiment moved while it holds the measured value", () => {
    expect([...retunedChecks([entry(measured)], [check(499, false)])]).toEqual(["b2\u0000fc_hz"]);
    expect(retunedChecks([entry(measured)], [check(500, false)]).size).toBe(1); // within 1%
  });

  it("ends when the check moves again, passes, or the experiment is undone", () => {
    expect(retunedChecks([entry(measured)], [check(300, false)]).size).toBe(0);
    expect(retunedChecks([entry(measured)], [check(996, true)]).size).toBe(0);
    expect(retunedChecks([entry({ ...measured, state: "undone" })], [check(499, false)]).size).toBe(0);
    expect(retunedChecks([entry({ ...measured, state: "applied", after: undefined })], [check(499, false)]).size).toBe(0);
  });

  it("leaves a check the experiment did not move", () => {
    const failing: Trial = { ...measured, before: [check(499, false)], after: [check(499, false)] };
    expect(retunedChecks([entry(failing)], [check(499, false)]).size).toBe(0);
  });
});
