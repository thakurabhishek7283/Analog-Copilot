import { expect, test } from "@playwright/test";

test("verified circuit produces a two-board guide; 3D edits never change 2D", async ({ page }) => {
  await page.goto("/#new");
  await expect(page).toHaveURL(/#p\/[0-9a-f-]{36}$/);
  await page.getByLabel("Describe a circuit").fill("e2e-verify a 300 Hz sine source through a 2 kHz RC low-pass filter and output buffer");
  await page.getByRole("button", { name: "Generate", exact: true }).click();
  await expect(page.locator(".progress")).toHaveAttribute("data-phase", "done", { timeout: 25_000 });
  await expect(page.locator(".progress .status")).toContainText("Circuit checked against your request");

  const id = page.url().split("#p/")[1]!;
  const snapshot = async () => {
    const token = await page.evaluate(() => localStorage.getItem("analog-copilot.session"));
    const response = await page.request.get(`/v1/projects/${id}`, { headers: { Authorization: `Bearer ${token}` } });
    expect(response.ok()).toBe(true);
    return response.json() as Promise<{ project: { head_rev: number }; circuit: { nets: unknown } }>;
  };
  const before = await snapshot();
  await page.getByRole("tab", { name: "3D breadboard" }).click();
  const board = page.getByRole("region", { name: "3D breadboard" });
  await board.getByRole("button", { name: "Generate build guide" }).click();
  await page.getByRole("tab", { name: "Schematic" }).click();
  await page.getByRole("tab", { name: "3D breadboard" }).click();
  await expect(board.getByText("✓ Wiring matches 2D")).toBeVisible({ timeout: 15_000 });
  await expect(board.locator("canvas")).toBeVisible();
  await expect(board.getByRole("button", { name: /U1 · board 2, row 5/ })).toBeVisible();
  await board.getByRole("button", { name: /^J1:/ }).click();
  await board.getByRole("button", { name: "Remove wire" }).click();
  await expect(board.getByText("Guide needs checking")).toBeVisible();
  await board.getByRole("button", { name: "Check guide" }).click();
  await expect(board.getByText("Guide invalid")).toBeVisible();
  await expect(board.getByRole("alert")).toContainText("connections differ");
  const after = await snapshot();
  expect(after.project.head_rev).toBe(before.project.head_rev);
  expect(after.circuit.nets).toEqual(before.circuit.nets);
});
