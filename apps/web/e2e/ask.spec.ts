// Ask against the real API (project `api`, see playwright.config.ts): questions about the circuit
// answered by the scripted model (apps/api/fake/script.json: a word in each question picks the
// reply), streamed over SSE, read against the circuit by the core, with an experiment tried and
// measured in the browser's own simulation.
import { expect, type Page, test } from "@playwright/test";
import { EditorPage } from "./editor.ts";

const panel = (page: Page) => page.locator('section[aria-label="Ask the tutor"]');
const entry = (page: Page) => panel(page).locator(".ask-entry").last();

async function newProject(page: Page): Promise<void> {
  await page.goto("/#new");
  await expect(page).toHaveURL(/#p\/[0-9a-f-]{36}$/);
  await expect(page.locator(".save-status")).toHaveText("Saved");
}

async function ask(page: Page, question: string): Promise<void> {
  await panel(page).getByLabel("Your question").fill(question);
  await panel(page).getByRole("button", { name: "Ask", exact: true }).click();
}

test("an answer about the selected part streams with chips, and its experiment is tried, measured and undone", async ({ page }) => {
  const editor = new EditorPage(page);
  await newProject(page);
  await page.getByLabel("Describe a circuit").fill("e2e-build a sine source into a 2 kHz low-pass filter, buffered");
  await page.getByRole("button", { name: "Generate" }).click();
  await page.locator(".progress .speed").getByRole("button", { name: "4×" }).click();
  await expect(page.locator(".progress")).toHaveAttribute("data-phase", "done", { timeout: 20_000 });
  await editor.settled();
  await editor.simulated();

  // Click a part, then ask about it: the question carries it, the chosen thinking, and the
  // browser's simulation of the circuit as saved.
  await editor.select("R1");
  await expect(panel(page).locator(".about")).toContainText("About R1");
  await panel(page).getByLabel("Thinking").selectOption({ label: "Deep thinking" });
  const request = page.waitForRequest((r) => r.url().endsWith("/ask") && r.method() === "POST");
  await ask(page, "e2e-ask why is R1 this value?");
  const body = (await request).postDataJSON();
  expect(body).toMatchObject({ question: "e2e-ask why is R1 this value?", selection: { kind: "part", refdes: "R1" }, effort: "high", mode: "explain" });
  expect(body.sim.status).toBe("ok");
  expect(body.sim.checks.map((c: { block: string; name: string }) => `${c.block}.${c.name}`)).toContain("b2.fc_hz");
  expect(Object.keys(body.sim.op_v).length).toBeGreaterThan(0);

  await expect(entry(page)).toHaveAttribute("data-phase", "done");
  const answer = entry(page).locator(".answer-text");
  await expect(answer).toContainText("set the corner of");
  await expect(answer).toContainText("R9 is not in this circuit."); // not a chip: plain text
  await expect(answer.locator(".ref-chip")).toHaveCount(5);
  await expect(answer.locator('.ref-chip[data-ref="R9"]')).toHaveCount(0);
  await expect(answer).not.toContainText("```try"); // the experiment is a card, not text
  await answer.locator('.ref-chip[data-ref="C1"]').first().click();
  await expect(page.locator(".inspector h2")).toHaveText("C1");

  // Predict, then test: one undo step, simulated, the moved check shown next to the prediction.
  const card = entry(page).locator(".try-card");
  await expect(card.locator(".ops")).toHaveText("R1 resistance → 100k");
  await expect(card.locator(".predict")).toContainText("the cutoff drops well below 2 kHz");
  const before = await page.locator('g.part[data-refdes="R1"] .value').first().textContent();
  await card.getByRole("button", { name: "Try it" }).click();
  await expect(page.locator('g.part[data-refdes="R1"] .value').first()).toHaveText("100kΩ");
  await expect(card).toHaveAttribute("data-state", "measured");
  await expect(card.locator(".measured tr")).toHaveCount(1);
  await expect(page.getByRole("button", { name: "Undo", exact: true })).toHaveAttribute("title", "Undo Try: the cutoff drops well below 2 kHz (Ctrl+Z)");
  await card.getByRole("button", { name: "Undo it" }).click();
  await expect(card).toHaveAttribute("data-state", "undone");
  await expect(page.locator('g.part[data-refdes="R1"] .value').first()).toHaveText(before!);

  const saved = page.waitForResponse((r) => r.url().endsWith("/feedback"));
  await entry(page).getByRole("button", { name: "Yes" }).click();
  expect((await saved).status()).toBe(204);
  await expect(entry(page).getByRole("button", { name: "Yes" })).toHaveAttribute("aria-pressed", "true");
  await expect(page.locator(".save-status")).toHaveText("Saved");
});

test("Socratic mode, Stop, and a failed answer that can be asked again", async ({ page }) => {
  await newProject(page);
  await panel(page).getByRole("button", { name: "Guide me" }).click();
  const request = page.waitForRequest((r) => r.url().endsWith("/ask"));
  await ask(page, "e2e-ask-slow what is this?");
  expect((await request).postDataJSON()).toMatchObject({ mode: "socratic" });
  expect((await request).postDataJSON()).not.toHaveProperty("effort");
  await expect(entry(page)).toContainText("Thinking…");
  await panel(page).getByRole("button", { name: "Stop" }).click();
  await expect(entry(page)).toHaveAttribute("data-phase", "stopped");

  await ask(page, "e2e-ask-fail what is this?");
  await expect(entry(page)).toHaveAttribute("data-phase", "failed");
  await expect(entry(page).locator(".ask-error")).toContainText("The model is not answering.");
  await expect(entry(page).getByRole("button", { name: "Ask again" })).toBeVisible();
  await expect(panel(page).locator(".ask-entry")).toHaveCount(2);
});

test("asking needs a saved project", async ({ page }) => {
  await new EditorPage(page).open("demo");
  await expect(panel(page)).toContainText("Asking needs a saved project");
  await expect(panel(page).getByLabel("Your question")).toHaveCount(0);
});
