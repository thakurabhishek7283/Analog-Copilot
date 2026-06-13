// LaTeX from real answers (the dry run and the tutor evals) as text a student can read.
import { describe, expect, it } from "vitest";
import { mathPlain, mathText } from "./latex.ts";

describe("mathText", () => {
  it("leaves text without LaTeX whole", () => {
    expect(mathText("fc = 1/(2π·R·C) ≈ 1.6 kHz, and B2_OUT costs $5")).toEqual([{ text: "fc = 1/(2π·R·C) ≈ 1.6 kHz, and B2_OUT costs $5" }]);
  });

  it("reads delimiters, fractions, roots, symbols and text", () => {
    expect(mathPlain(String.raw`\(1/\sqrt{C2}\)`)).toBe("1/√C2");
    expect(mathPlain(String.raw`\(9\times4.9\)`)).toBe("9×4.9");
    expect(mathPlain(String.raw`\(\frac{1}{2\pi RC}\) \approx 1\,\text{kHz}`)).toBe("1/(2π RC) ≈ 1 kHz");
    expect(mathPlain(String.raw`\(Q=\sqrt{R1R2C1C2}/[C2(R1+R2)]\)`)).toBe("Q=√(R1R2C1C2)/[C2(R1+R2)]");
    expect(mathPlain(String.raw`\(12V-(1.73\text{mA}\times5.6\text{kΩ})\approx2.3V\)`)).toBe("12V-(1.73mA×5.6kΩ)≈2.3V");
    expect(mathPlain(String.raw`\(\frac{\sqrt{2}}{2}\)`)).toBe("√2/2");
    expect(mathPlain(String.raw`a phase of \(-45^\circ\) and $90^{\circ}$`)).toBe("a phase of -45° and 90°");
  });

  it("keeps subscripts and superscripts apart, and names with underscores whole", () => {
    expect(mathText(String.raw`\(A_v \approx -9.7\)`)).toEqual([{ text: "A" }, { sub: "v" }, { text: " ≈ -9.7" }]);
    expect(mathText(String.raw`\(2\pi fV_{\text{peak}}\approx0.0063\ \text{V}/\mu\text{s}\)`)).toEqual([
      { text: "2π fV" }, { sub: "peak" }, { text: "≈0.0063 V/µs" },
    ]);
    expect(mathText(String.raw`\(f_c\) at \(10^{-3}\) and \(R^2\)`)).toEqual([
      { text: "f" }, { sub: "c" }, { text: " at 10" }, { sup: "-3" }, { text: " and R" }, { sup: "2" },
    ]);
    expect(mathPlain(String.raw`\(V_{B2_OUT}\) on B2_OUT \(\times 2\)`)).toBe("VB2_OUT on B2_OUT × 2");
  });

  it("drops an unknown command's backslash and stray braces", () => {
    expect(mathPlain(String.raw`\(\mathcal{L}\{x\}\)`)).toBe("mathcalLx");
  });
});
