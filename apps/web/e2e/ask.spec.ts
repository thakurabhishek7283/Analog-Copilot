// Ask against the real API (project `api`, see playwright.config.ts): questions about the circuit
// answered by the scripted model (apps/api/fake/script.json: a word in each question picks the
// reply), streamed over SSE, read against the circuit by the core, with an experiment tried and
// measured in the browser's own simulation, then explained ("What changed?").
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

test("an answer about the selected part streams with chips, and its experiment is tried, measured, explained and undone", async ({ page }) => {
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
  await panel(page).getByLabel("Thinking").selectOption({ label: "Deep" });
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
  await expect(answer.locator(".ref-chip")).toHaveCount(6);
  // LaTeX the model slipped in reads as text; the block named in its own sentence is a compact chip.
  await expect(answer).toContainText("= 1/(2π RC)");
  await expect(answer.locator("sub")).toHaveText("c");
  await expect(answer).not.toContainText("\\frac");
  await expect(answer.locator('.ref-chip[data-ref="b2"]').first()).toHaveText("RC low-pass filter");
  await expect(answer.locator('.ref-chip[data-ref="b2"][data-compact]')).toHaveText("⌖");
  await expect(answer.locator('.ref-chip[data-ref="R9"]')).toHaveCount(0);
  await expect(answer).not.toContainText("```try"); // the experiment is a card, not text
  await answer.locator('.ref-chip[data-ref="C1"]').first().click();
  await expect(page.locator(".inspector h2")).toHaveText("C1");

  // Predict, then test: one undo step, simulated, the moved check shown next to the prediction.
  const first = panel(page).locator(".ask-entry").first();
  const card = first.locator(".try-card");
  await expect(card.locator(".ops")).toHaveText("R1 resistance → 16k");
  await expect(card.locator(".predict")).toContainText("the cutoff drops to about 1 kHz");
  const before = await page.locator('g.part[data-refdes="R1"] .value').first().textContent();
  await card.getByRole("button", { name: "Try it" }).click();
  await expect(page.locator('g.part[data-refdes="R1"] .value').first()).toHaveText("16kΩ");
  await expect(card).toHaveAttribute("data-state", "measured");
  await expect(card.locator(".measured tr")).toHaveCount(1);
  // The prediction held, and the cutoff it moved off its 2 kHz target on purpose is not a failure.
  await expect(card.locator(".verdict.held")).toContainText("✓ Prediction held: predicted 1 kHz, measured");
  const badge = page.locator('g.block[data-block="b2"] .badge[data-check="fc_hz"]');
  await expect(badge).toHaveClass(/retuned/);
  await expect(badge).toContainText("(retuned)");
  await expect(page.getByRole("button", { name: "Undo", exact: true })).toHaveAttribute("title", "Undo Try: the cutoff drops to about 1 kHz (Ctrl+Z)");

  // What changed? The server compares its circuits at both revs; the browser sends its simulation
  // of each, and the cutoff check that moved is in both.
  await expect(panel(page).locator("button.what-changed")).toContainText("Try: the cutoff drops to about 1 kHz");
  const asked = page.waitForRequest((r) => r.url().endsWith("/what-changed") && r.method() === "POST");
  await card.getByRole("button", { name: "What changed?" }).click();
  const change = (await asked).postDataJSON();
  expect(change.rev).toBe(change.from_rev + 1);
  const fc = (sim: { checks: { block: string; name: string; measured?: number }[] }) => sim.checks.find((c) => c.block === "b2" && c.name === "fc_hz")!.measured;
  expect(fc(change.after)).toBeLessThan(fc(change.before)!);
  const explained = entry(page);
  await expect(explained).toHaveAttribute("data-phase", "done");
  await expect(explained.locator(".question")).toHaveText("What changed? · Try: the cutoff drops to about 1 kHz");
  await expect(explained.locator(".answer-text")).toContainText("You changed");
  await expect(explained.locator('.answer-text .ref-chip[data-ref="R1"]')).toHaveCount(1);
  await expect(explained.locator('.answer-text .ref-chip[data-ref="b2"]')).toHaveCount(1);
  await expect(panel(page).locator("button.what-changed")).toHaveCount(0); // asked already

  await card.getByRole("button", { name: "Undo it" }).click();
  await expect(card).toHaveAttribute("data-state", "undone");
  await expect(page.locator('g.part[data-refdes="R1"] .value').first()).toHaveText(before!);
  await editor.simulated();
  await expect(badge).toHaveClass(/pass/);
  await expect(panel(page).locator("button.what-changed")).toContainText("Undo Try: the cutoff drops to about 1 kHz");

  const saved = page.waitForResponse((r) => r.url().endsWith("/feedback"));
  await first.getByRole("button", { name: "Yes" }).click();
  expect((await saved).status()).toBe(204);
  await expect(first.getByRole("button", { name: "Yes" })).toHaveAttribute("aria-pressed", "true");
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

test("a follow-up is sent with the conversation so far, until a new topic", async ({ page }) => {
  await newProject(page);
  // An empty panel offers questions to start from; one fills the box, the learner sends it.
  await panel(page).getByRole("button", { name: "What does this circuit do?" }).click();
  await expect(panel(page).getByLabel("Your question")).toHaveValue("What does this circuit do?");
  await expect(panel(page).locator(".examples")).toBeVisible(); // nothing was asked yet
  await ask(page, "what is this circuit?");
  await expect(panel(page).locator(".examples")).toHaveCount(0);
  await expect(entry(page)).toHaveAttribute("data-phase", "done");
  const follow = page.waitForRequest((r) => r.url().endsWith("/ask"));
  await ask(page, "can you say that more simply?");
  expect((await follow).postDataJSON().history).toEqual([expect.stringMatching(/^[0-9a-f-]{36}$/)]);
  await expect(entry(page)).toHaveAttribute("data-phase", "done"); // the server found the earlier answer

  await panel(page).getByRole("button", { name: "New topic" }).click();
  await expect(panel(page).locator(".topic-break")).toHaveCount(1);
  await expect(panel(page).getByRole("button", { name: "New topic" })).toHaveCount(0);
  const fresh = page.waitForRequest((r) => r.url().endsWith("/ask"));
  await ask(page, "something else?");
  expect((await fresh).postDataJSON()).not.toHaveProperty("history");
  await expect(entry(page)).toHaveAttribute("data-phase", "done");
});

test("asking needs a saved project", async ({ page }) => {
  await new EditorPage(page).open("demo");
  await expect(panel(page)).toContainText("Asking needs a saved project");
  await expect(panel(page).getByLabel("Your question")).toHaveCount(0);
});
