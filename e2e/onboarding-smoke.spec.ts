import { test, expect } from "@playwright/test";
import { TestApiClient } from "./fixtures";
import { waitForPageText } from "./helpers";

// Smoke test for the onboarding flow: welcome → workspace → runtime.
// The questionnaire is intentionally absent. Captures screenshots for
// review. Uses a unique email per run so the user is always a fresh,
// un-onboarded user landing on /onboarding.

const EMAIL = `onboarding-v3-${Date.now()}@localhost`;
const SHOTS_DIR = "../shots-rail";

test.use({ viewport: { width: 1440, height: 900 } });

test("onboarding — welcome → workspace", async ({ page }) => {
  const api = new TestApiClient();
  await api.login(EMAIL, "OBv3 Tester");
  const token = api.getToken();

  await page.addInitScript((t) => {
    localStorage.setItem("patchbay_token", t);
  }, token);
  await page.goto("/onboarding", { waitUntil: "domcontentloaded" });
  await waitForPageText(page, "Continue on web");

  await expect(page.getByRole("button", { name: "Continue on web" })).toBeVisible({ timeout: 15000 });
  await page.screenshot({ path: `${SHOTS_DIR}/01-welcome.png`, fullPage: false });

  await page.getByRole("button", { name: "Continue on web" }).click();

  await expect(
    page.getByRole("heading", { name: /Set up your first workspace/i }),
  ).toBeVisible({ timeout: 10000 });
  await expect(page.getByText("Tell us a bit about you.")).toHaveCount(0);
  await expect(page.locator('[data-slot="stepper-title"]')).toHaveText([
    "Workspace",
    "Meet Mika",
  ]);
  await expect(
    page.locator('[aria-current="step"]').filter({ hasText: "Workspace" }),
  ).toBeVisible();
  await expect(page.getByText("How did you hear about Patchbay?")).toHaveCount(0);
  await page.waitForTimeout(500);
  await page.screenshot({ path: `${SHOTS_DIR}/03-workspace.png` });

  await page.getByRole("textbox").first().fill(`Rail QA ${Date.now()}`);
  await page.getByRole("button", { name: /^Create /i }).click();
  await expect(
    page.locator('[aria-current="step"]').filter({ hasText: "Meet Mika" }),
  ).toBeVisible({ timeout: 20000 });
  await page.waitForTimeout(800);
  await page.screenshot({ path: `${SHOTS_DIR}/06-runtime.png` });
});

test("onboarding — zh-Hans renders Chinese labels", async ({ page, context, baseURL }) => {
  await context.addCookies([
    {
      name: "patchbay-locale",
      value: "zh-Hans",
      url: baseURL ?? "http://localhost:3000",
    },
  ]);
  const api = new TestApiClient();
  await api.login(`zh-${Date.now()}@localhost`, "中文用户");
  const token = api.getToken();

  await page.addInitScript((t) => localStorage.setItem("patchbay_token", t), token);
  await page.goto("/onboarding", { waitUntil: "domcontentloaded" });
  await waitForPageText(page, "在 web 端继续");

  await page.getByRole("button", { name: "在 web 端继续" }).click();

  await expect(page.getByRole("heading", { name: "设置你的第一个工作区" })).toBeVisible({
    timeout: 10000,
  });
  await expect(page.getByText("简单介绍一下你自己。")).toHaveCount(0);
  await page.waitForTimeout(500);
  await page.screenshot({ path: `${SHOTS_DIR}/05-workspace-zh.png` });
});
