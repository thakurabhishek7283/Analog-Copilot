import { describe, expect, it } from "vitest";
import type { PartDef, PartInstance } from "../../gen/contract.ts";
import { formatNet, formatSi, formatVolts, netTitle, partValue } from "./labels.ts";

const q = (si: number, display: string) => ({ si, display, unit: "volt" as const });

describe("labels", () => {
  it("formats voltages to three significant figures", () => {
    expect(formatVolts(12)).toBe("12 V");
    expect(formatVolts(-11.987)).toBe("-12 V");
    expect(formatVolts(1.23456)).toBe("1.23 V");
    expect(formatVolts(0.1)).toBe("100 mV");
    expect(formatVolts(-0.0123)).toBe("-12.3 mV");
    expect(formatVolts(1e-6)).toBe("0 V");
    expect(formatVolts(0.99999)).toBe("1 V");
    expect(formatVolts(-0.9996)).toBe("-1 V");
    expect(formatVolts(0.9994)).toBe("999 mV");
  });

  it("shows non-zero params, or the type number", () => {
    const sine = { title: "Sine signal source", params: { offset: {}, amplitude: {}, frequency: {} } } as unknown as PartDef;
    const inst = {
      params: { amplitude: q(1, "1V"), frequency: { si: 1000, display: "1kHz", unit: "hertz" }, offset: q(0, "0V") },
    } as unknown as PartInstance;
    expect(partValue(inst, sine)).toBe("1V 1kHz");
    const opamp = { title: "TL072 dual JFET op-amp", params: {} } as unknown as PartDef;
    expect(partValue({ params: {} } as unknown as PartInstance, opamp)).toBe("TL072");
    const led = { title: "Red LED, 5 mm", params: {} } as unknown as PartDef;
    expect(partValue({ params: {} } as unknown as PartInstance, led)).toBe("");
  });
});

describe("formatSi", () => {
  it("picks a prefix and keeps three significant figures", () => {
    expect(formatSi(0.0047, "A")).toBe("4.7 mA");
    expect(formatSi(-2.5e-6, "A")).toBe("-2.5 µA");
    expect(formatSi(1000, "Hz")).toBe("1 kHz");
    expect(formatSi(0.0049996, "s")).toBe("5 ms");
    expect(formatSi(12, "V")).toBe("12 V");
    expect(formatSi(0, "V")).toBe("0 V");
    expect(formatSi(Number.NaN, "V")).toBe("—");
  });
});

describe("formatNet", () => {
  it("shows a signal's swing, and a steady net's operating point", () => {
    expect(formatNet(0, { min: -0.705, max: 0.705 })).toBe("±705 mV"); // a sine centred on zero
    expect(formatNet(6.63, { min: 5.63, max: 7.63 })).toBe("6.63 V ±1 V"); // a signal on a bias
    expect(formatNet(12, { min: 12, max: 12 })).toBe("12 V"); // a rail
    expect(formatNet(2.5, { min: 2.4999, max: 2.5001 })).toBe("2.5 V"); // a ripple under 1 mV
    expect(formatNet(0.0024, undefined)).toBe("2.4 mV"); // no transient
    expect(formatNet(undefined, undefined)).toBeUndefined();
    expect(netTitle(12, { min: 12, max: 12 })).toBe("12 V at the operating point (no signal)"); // no "swings 12 V to 12 V"
    expect(netTitle(0, { min: -0.705, max: 0.705 })).toBe(
      "swings -705 mV to 705 mV in the transient (after start-up); 0 V at the operating point (no signal)",
    );
  });
});
