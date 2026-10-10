import { expect, test } from "@playwright/test";
import { EditorPage } from "./editor.ts";

test("a simulated demo does not create a build guide without final verification", async ({ page }) => {
  const editor = new EditorPage(page);
  await editor.open("demo");
  await page.getByRole("tab", { name: "3D breadboard" }).click();
  const board = page.getByRole("region", { name: "3D breadboard" });
  await expect(board.getByText("Requires a finalized, passed 2D verification")).toBeVisible();
  await expect(board.getByRole("button", { name: "Generate build guide" })).toBeDisabled();
  await expect(board.locator("canvas")).toHaveCount(0);
});
