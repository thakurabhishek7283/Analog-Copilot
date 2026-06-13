// An answer as the Ask panel shows it: chips at the core's UTF-16 spans, invalid references as
// plain text, ops in words, and the checks an experiment moved.
import { describe, expect, it } from "vitest";
import type { CheckResult, Ref } from "../gen/contract.ts";
import { changedChecks, compactBlocks, describeOp, segments } from "./answer.ts";

const ref = (body: string, token: string, kind: Ref["kind"], id: string, valid = true): Ref => {
  const start = body.indexOf(token);
  return { kind, id, valid, start, end: start + token.length };
};

describe("segments", () => {
  it("cuts at UTF-16 spans, past characters outside the BMP", () => {
    const body = "𝜏 = [R1]·[C1] on [net:N_A]; [R9] is not here.";
    const refs = [ref(body, "[net:N_A]", "net", "N_A"), ref(body, "[R1]", "part", "R1"), ref(body, "[C1]", "part", "C1"), ref(body, "[R9]", "part", "R9", false)];
    expect(segments(body, refs)).toEqual([
      { text: "𝜏 = " },
      { ref: refs[1] },
      { text: "·" },
      { ref: refs[2] },
      { text: " on " },
      { ref: refs[0] },
      { text: "; R9 is not here." }, // invalid: plain text, merged with its neighbours
    ]);
  });

  it("keeps text intact when spans overlap or run past the end", () => {
    const body = "See [R1].";
    const r = ref(body, "[R1]", "part", "R1");
    expect(segments(body, [r, { ...r }, { ...r, start: 2, end: 99 }])).toEqual([{ text: "See " }, { ref: r }, { text: "." }]);
    expect(segments("plain", [])).toEqual([{ text: "plain" }]);
  });
});

describe("describeOp", () => {
  it("puts each learner op in words", () => {
    expect(describeOp({ op: "part.set_param", body: { refdes: "R1", key: "resistance", value: "36k" } })).toBe("R1 resistance → 36k");
    expect(describeOp({ op: "part.add", body: { refdes: "R9", part: "resistor_th", params: { resistance: "1k" } } })).toBe("Add R9 (resistor_th, resistance 1k)");
    expect(describeOp({ op: "net.connect", body: { net: "N_A", pins: ["R9.1"] } })).toBe("Connect R9.1 to N_A");
    expect(describeOp({ op: "analysis.set", body: { analyses: [{ type: "op" }, { type: "ac", points_per_decade: 20, f_start: 10, f_stop: 1e5 }] } })).toBe("Run OP, AC");
  });
});

describe("changedChecks", () => {
  const check = (name: string, measured: string | null): CheckResult => ({
    block: "b2", name, label: name, symbol: name, unit: "hertz", target: 1000, tol_pct: 10, pass: true, target_display: "1kHz",
    measured_display: measured,
  });
  it("lists the checks whose measured value moved", () => {
    const before = [check("fc_hz", "996Hz"), check("q", "0.706")];
    const after = [check("fc_hz", "499Hz"), check("q", "0.706"), check("gain", null)];
    expect(changedChecks(before, after)).toEqual([{ block: "b2", name: "fc_hz", label: "fc_hz", before: "996Hz", after: "499Hz", pass: true, target: "1kHz" }]);
    expect(changedChecks([], [check("fc_hz", "1kHz")])).toEqual([{ block: "b2", name: "fc_hz", label: "fc_hz", before: "—", after: "1kHz", pass: true, target: "1kHz" }]);
  });
});

describe("compactBlocks", () => {
  const titles: Record<string, string> = { b2: "Sallen-Key low-pass (2nd order)", b3: "Inverting amplifier" };
  const compact = (body: string) => {
    const refs = [...body.matchAll(/\[block:(b\d)\]/g)].map((m) => ({ kind: "block" as const, id: m[1]!, valid: true, start: m.index!, end: m.index! + m[0].length }));
    const segs = segments(body, refs);
    return [...compactBlocks(segs, (id) => titles[id])].map((i) => (segs[i] as { ref: Ref }).ref.id);
  };

  it("shows the title once, and not after the model's own words for the block", () => {
    expect(compact("The Sallen-Key filter [block:b2] sets fc.")).toEqual(["b2"]);
    expect(compact("This sets the corner of [block:b2]. Later [block:b2] again.")).toEqual(["b2"]);
    expect(compact("[block:b2] feeds the amplifier [block:b3].")).toEqual(["b3"]);
    expect(compact("The low-pass part. Then [block:b3] inverts it.")).toEqual([]);
  });
});
