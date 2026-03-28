// The panel layout (Workspace.tsx, panels.ts): the learner drags the borders, collapses panes and
// focuses the tutor; the layout is kept in this browser across a reload, and Reset layout puts the
// defaults back.
import { expect, type Locator, type Page, test } from "@playwright/test";
import { EditorPage } from "./editor.ts";

const width = async (l: Locator) => (await l.boundingBox())!.width;
const height = async (l: Locator) => (await l.boundingBox())!.height;

/** Drag a handle (it takes no space: its grab area is a pseudo-element over the border). */
async function drag(page: Page, label: string, dx: number, dy: number): Promise<void> {
  const box = (await page.getByRole("separator", { name: label }).boundingBox())!;
  const x = box.x + box.width / 2;
  const y = box.y + box.height / 2;
  await page.mouse.move(x, y);
  await page.mouse.down();
  await page.mouse.move(x + dx / 2, y + dy / 2);
  await page.mouse.move(x + dx, y + dy);
  await page.mouse.up();
}

test("the learner resizes, collapses and focuses panels, and the layout survives a reload until reset", async ({ page }) => {
  const editor = new EditorPage(page);
  await editor.open("demo");
  const side = page.locator("main > .side");
  const palette = page.locator("main > .palette");
  const inspector = page.locator('.pane[data-pane="inspector"]');
  const toggle = (pane: string) => page.locator(`.pane[data-pane="${pane}"] .pane-toggle`);
  await expect(page.getByRole("button", { name: "Reset layout" })).toHaveCount(0);
  expect(await width(side)).toBeCloseTo(400, -1);

  // Wider tutor column, narrower parts column.
  await drag(page, "Resize the side column", -200, 0);
  expect(await width(side)).toBeCloseTo(600, -1);
  await drag(page, "Resize the parts column", -40, 0);
  expect(await width(palette)).toBeCloseTo(160, -1);

  // The scope from the keyboard: two steps up make it 32 px taller.
  const scope = page.locator(".scope-body");
  const before = await height(scope);
  await page.getByRole("separator", { name: "Resize the scope" }).focus();
  await page.keyboard.press("ArrowUp");
  await page.keyboard.press("ArrowUp");
  expect(await height(scope)).toBeCloseTo(before + 32, 0);

  // Collapse the inspector: the tutor takes its height.
  const ask = page.locator('.pane[data-pane="ask"]');
  const askBefore = await height(ask);
  await toggle("inspector").click();
  await expect(toggle("inspector")).toHaveAttribute("aria-expanded", "false");
  await expect(page.locator(".inspector")).toBeHidden();
  expect(await height(ask)).toBeGreaterThan(askBefore + 100);

  // Kept across a reload.
  await page.waitForTimeout(300); // saved shortly after it stops changing
  await page.reload();
  await editor.simulated();
  expect(await width(side)).toBeCloseTo(600, -1);
  await expect(toggle("inspector")).toHaveAttribute("aria-expanded", "false");

  // Insert block opens its form in the inspector, so the inspector opens for it.
  await page.click('button[data-template="sallen_key_lp"]');
  await expect(page.getByRole("form", { name: "Insert Sallen-Key low-pass (2nd order)" })).toBeVisible();
  await expect(toggle("inspector")).toHaveAttribute("aria-expanded", "true");

  // Focus: the tutor gets most of the window; Exit focus puts the layout back.
  await page.getByRole("button", { name: "Focus", exact: true }).click();
  await expect(page.locator("main > .palette.collapsed")).toBeVisible();
  await expect(inspector.locator(".pane-body")).toBeHidden();
  expect(await width(side)).toBeGreaterThanOrEqual(600);
  await page.getByRole("button", { name: "Exit focus" }).click();
  await expect(palette.locator(".palette-head")).toBeVisible();
  expect(await width(palette)).toBeCloseTo(160, -1);
  await expect(inspector.locator(".pane-body")).toBeVisible();

  // The palette hides to a strip and comes back.
  await page.getByRole("button", { name: "Hide tools and parts" }).click();
  expect(await width(page.locator("main > .palette"))).toBeLessThan(40);
  await page.getByRole("button", { name: "Show tools and parts" }).click();

  await page.getByRole("button", { name: "Reset layout" }).click();
  expect(await width(side)).toBeCloseTo(400, -1);
  expect(await width(palette)).toBeCloseTo(200, -1);
  await expect(page.getByRole("button", { name: "Reset layout" })).toHaveCount(0);
});

test("a double click on a handle puts that size back", async ({ page }) => {
  await new EditorPage(page).open("demo");
  const side = page.locator("main > .side");
  await drag(page, "Resize the side column", -120, 0);
  expect(await width(side)).toBeCloseTo(520, -1);
  const box = (await page.getByRole("separator", { name: "Resize the side column" }).boundingBox())!;
  await page.mouse.dblclick(box.x + box.width / 2, box.y + box.height / 2);
  expect(await width(side)).toBeCloseTo(400, -1);
});
