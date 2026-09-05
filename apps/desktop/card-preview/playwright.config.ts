// eslint-disable-next-line import-x/no-extraneous-dependencies -- Playwright config is dev tooling; dependency is declared in desktop devDependencies.
import { defineConfig } from "@playwright/test";

export default defineConfig({
  testDir: ".",
  testMatch: "preview.spec.ts",
  outputDir: "../../../test-results/card-preview",
  use: {
    baseURL: "http://127.0.0.1:5188",
    channel: process.env.PLAYWRIGHT_CHANNEL || undefined,
    screenshot: "only-on-failure",
  },
  webServer: {
    command: "pnpm dev:cards",
    cwd: "../../..",
    url: "http://127.0.0.1:5188",
    reuseExistingServer: !process.env.CI,
    timeout: 120_000,
  },
});
