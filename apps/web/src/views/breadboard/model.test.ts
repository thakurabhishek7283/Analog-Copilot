import { describe, expect, it } from "vitest";
import { holePosition, strip } from "./model.ts";

describe("physical board coordinates", () => {
  it("separates boards, sides, and external terminals", () => {
    expect(strip("B1:A12")).toBe(strip("B1:E12"));
    expect(strip("B1:F12")).not.toBe(strip("B1:E12"));
    expect(strip("B2:E12")).not.toBe(strip("B1:E12"));
    expect(strip("X:V1.P")).toBe("X:V1.P");
    expect(holePosition("B2:E12")[0] - holePosition("B1:E12")[0]).toBe(13);
  });

  it("rejects holes outside a standard 63-row board", () => {
    expect(() => strip("B1:E64")).toThrow();
    expect(() => holePosition("B9:E1")).toThrow();
  });
});
