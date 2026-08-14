import { expect, test } from "@playwright/test";

test.describe("private download flow", () => {
  test.skip(!process.env.MOVIEBOX_E2E_URL, "Requires a local fixture server; CI supplies this when enabled.");

  test("login, queue a capped fixture, and observe ready state", async ({ page }) => {

  await page.goto("/");
  await page.getByLabel(/server password/i).fill("fixture-secret");
  await page.getByRole("button", { name: /sign in/i }).click();
  await page.getByLabel("Find a film or series").fill("Fixture Movie");
  await expect(page.getByRole("button", { name: "Fixture Movie" })).toBeVisible();
  await page.getByRole("button", { name: "Fixture Movie" }).click();
  await page.getByLabel("1080p").check();
  await page.getByRole("button", { name: "Download" }).click();
  await page.getByRole("button", { name: "Confirm" }).click();
  await expect(page.getByText("Ready")).toBeVisible({ timeout: 30_000 });
  await expect(page.getByRole("link", { name: "Open in Jellyfin" })).toBeVisible();
  });
});
