import { test, expect } from "@playwright/test";

test("real card state changes remain local, including hover", async ({
  page,
}) => {
  const errors: string[] = [];
  const requests: string[] = [];
  page.on("pageerror", (error) => errors.push(error.message));
  page.on("request", (request) => {
    if (
      ["fetch", "xhr"].includes(request.resourceType()) ||
      request.method() !== "GET"
    ) {
      requests.push(request.url());
    }
  });
  await page.goto("/");
  const card = page.getByTestId("preview-card");
  await expect(card.getByText("Working", { exact: true })).toBeVisible();
  await expect(card.locator(".animate-chat-text-shimmer")).toHaveCSS(
    "animation-name",
    /shimmer/,
  );
  await page.getByLabel("任务状态", { exact: true }).selectOption("in_review");
  await expect(card.getByText("Working", { exact: true })).toBeVisible();
  await card.getByText("Working", { exact: true }).hover();
  await expect(page.getByText("1 task working", { exact: true })).toBeVisible();
  await page.screenshot({
    path: "test-results/card-preview/light-running.png",
  });
  await page.getByRole("heading", { name: "卡片状态预览" }).hover();
  await page.getByLabel("模拟执行状态").selectOption("idle");
  await expect(card.locator(".animate-chat-text-shimmer")).toHaveCount(0);
  await expect(card.getByText("Working", { exact: true })).toHaveCount(0);
  await page.getByLabel("模拟执行状态").selectOption("queued");
  await expect(card.getByText("Queued", { exact: true })).toBeVisible();
  await expect(card.locator(".animate-chat-text-shimmer")).toHaveCount(0);
  await page.getByLabel("主题").selectOption("dark");
  await expect(page.locator("main")).toHaveClass("dark preview");
  await page.setViewportSize({ width: 375, height: 812 });
  expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBe(
    375,
  );
  await page.screenshot({ path: "test-results/card-preview/dark-mobile.png" });
  expect(errors).toEqual([]);
  expect(requests).toEqual([]);
});

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
