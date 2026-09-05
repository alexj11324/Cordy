import { test, expect } from "@playwright/test";

test("browser policy blocks even direct requests", async ({ page }) => {
  const response = await page.goto("/");
  expect(response?.headers()["content-security-policy"]).toContain(
    "connect-src 'none'",
  );
  await expect(page.getByTestId("preview-card")).toBeVisible();
  const blocked = await page.evaluate(async () => {
    try {
      await fetch("/api/issues", { method: "POST", body: "{}" });
      return false;
    } catch {
      return true;
    }
  });
  expect(blocked).toBe(true);
});
