// A library component end to end (LLD §17, project `api`): its definition and an instance are
// sent through `POST /ops` like any user edit, the server applies them with circuit-core and saves
// them, and the editor that opens the project draws the instance with its generated symbol and
// simulates it in the browser.
import { expect, test } from "@playwright/test";
import { EditorPage } from "./editor.ts";

const pin = (name: string, num: number, type: string) => ({ name, num, type });
const buffer = {
  part: {
    id: "lib:buf",
    category: "X",
    prefix: "U",
    title: "Buffer",
    symbol: "auto",
    pins: [pin("IN", 1, "input"), pin("OUT", 2, "output"), pin("VCC", 3, "power_pos"), pin("VEE", 4, "power_neg")],
  },
  version: 1,
  model: {
    kind: "composite",
    parts: { U1: { part: "opamp_tl072" } },
    nets: { IN: ["U1.INP_A"], OUT: ["U1.OUT_A", "U1.INM_A"], VCC: ["U1.VCC"], VEE: ["U1.VEE"] },
  },
  provenance: { kind: "user" },
};

const ops: [string, unknown][] = [
  ["lib.add", { def: buffer }],
  ["part.add", { refdes: "V1", part: "vsource_sine", params: { amplitude: "1", frequency: "1k" } }],
  ["net.connect", { net: "N_IN", pins: ["V1.P"] }],
  ["net.connect", { net: "GND", pins: ["V1.N"] }],
  ["part.add", { refdes: "U1", part: "lib:buf" }],
  ["net.connect", { net: "N_IN", pins: ["U1.IN"] }],
  ["net.connect", { net: "N_OUT", pins: ["U1.OUT"] }],
  ["net.connect", { net: "VCC", pins: ["U1.VCC"], kind: { kind: "power", volts: 12 } }],
  ["net.connect", { net: "VEE", pins: ["U1.VEE"], kind: { kind: "power", volts: -12 } }],
  ["part.add", { refdes: "R1", part: "resistor_th", params: { resistance: "10k" } }],
  ["net.connect", { net: "N_OUT", pins: ["R1.1"] }],
  ["net.connect", { net: "GND", pins: ["R1.2"] }],
];

test("a library component sent through the API is drawn with its own symbol and simulates", async ({ page }) => {
  const editor = new EditorPage(page);
  await page.goto("/#new");
  await expect(page).toHaveURL(/#p\/[0-9a-f-]{36}$/);
  await expect(page.locator(".save-status")).toHaveText("Saved");
  const project = page.url().split("#p/")[1]!;
  const token = await page.evaluate(() => localStorage.getItem("analog-copilot.session"));
  const headers = { Authorization: `Bearer ${token}` };

  const snap = await (await page.request.get(`/v1/projects/${project}`, { headers })).json();
  const base = snap.circuit.rev as number;
  const envelopes = ops.map(([op, body], i) => ({ v: 1, seq: base + i + 1, op, author: "user", base_rev: base + i, body }));
  const r = await page.request.post(`/v1/projects/${project}/ops`, { headers, data: { base_rev: base, ops: envelopes } });
  expect(r.status(), await r.text()).toBe(200);

  await page.reload();
  await editor.settled();
  await editor.simulated();
  // The generated symbol is in the page and the instance uses it.
  await expect(page.locator("symbol#sym-lib_buf")).toBeAttached();
  await expect(page.locator('g.part[data-refdes="U1"] use')).toHaveAttribute("href", "#sym-lib_buf");
  // Its pins are where wires snap (the wire tool shows them).
  await editor.tool("w");
  await editor.pin("U1.IN");
  await editor.pin("U1.OUT");
  await page.keyboard.press("Escape");
  await editor.select("U1");
  await expect(page.locator(".inspector")).toContainText("Buffer");
  if (process.env.E2E_SHOT) await page.screenshot({ path: process.env.E2E_SHOT });
});
