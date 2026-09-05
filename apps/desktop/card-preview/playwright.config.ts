// eslint-disable-next-line import-x/no-extraneous-dependencies -- Playwright config is dev tooling; dependency is declared in desktop devDependencies.
import { defineConfig } from "@playwright/test";
import { previewUrl } from "./port";

export default defineConfig({
  testDir: "../../..",
  testMatch: [
    "apps/desktop/card-preview/preview.spec.ts",
    "packages/views/issues/components/board-card.browser.spec.ts",
  ],
  outputDir: "../../../test-results/card-preview",
  use: {
    baseURL: previewUrl,
    channel: process.env.PLAYWRIGHT_CHANNEL || undefined,
    screenshot: "only-on-failure",
  },
  webServer: {
    command: "pnpm dev:cards",
    cwd: "../../..",
    url: previewUrl,
    reuseExistingServer: !process.env.CI,
    timeout: 120_000,
  },
});
