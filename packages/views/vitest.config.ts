import { defineConfig } from "vitest/config";
import react from "@vitejs/plugin-react";

export default defineConfig({
  plugins: [react()],
  test: {
    environment: "jsdom",
    globals: true,
    setupFiles: ["./test/setup.ts"],
    include: ["**/*.test.{ts,tsx}"],
    // These packages ship ESM that imports JSON without an import attribute
    // (`@emoji-mart/data`). Node's ESM loader rejects that outright; Vite's
    // transform pipeline handles it. Inlining routes them through Vite instead
    // of externalising them to Node. `@lobehub/editor` reaches the same JSON
    // through its own dependency chain, so it needs the same treatment.
    server: {
      deps: {
        inline: ["@lobehub/ui", "@lobehub/editor"],
      },
    },
    // The chat rows now render through LobeHub's components, which run on
    // antd. antd resolves its CSS-in-JS style tree per render, and under jsdom
    // that costs seconds per suite where our Tailwind components cost
    // milliseconds — enough to blow the 5s default once several suites run
    // concurrently. A real browser is unaffected; this budget is for the test
    // environment only.
    testTimeout: 20_000,
  },
});
