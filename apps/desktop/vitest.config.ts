import { resolve } from "path";
import { defineConfig } from "vitest/config";
import react from "@vitejs/plugin-react";

export default defineConfig({
  plugins: [react()],
  resolve: {
    alias: {
      "@": resolve(__dirname, "src/renderer/src"),
    },
  },
  test: {
    globals: true,
    include: ["src/**/*.test.{ts,tsx}", "scripts/**/*.test.mjs"],
    environment: "jsdom",
    setupFiles: ["./test/setup.ts"],
    passWithNoTests: true,
    // Same reason as `packages/views/vitest.config.ts`, and it has to be
    // repeated here because Vitest reads the config of the package it runs in:
    // the LobeHub packages ship ESM that imports JSON with no import attribute
    // (`@emoji-mart/data`, reached through `@lobehub/ui`'s EmojiPicker). Node's
    // ESM loader rejects that outright; Vite's transform pipeline handles it.
    // Inlining routes them through Vite instead of externalising them.
    //
    // This became necessary when the settings surface moved onto LobeHub: the
    // desktop renderer's route graph reaches `@orvilo/views/settings`, which now
    // imports `@lobehub/ui`, so suites that never touched a Lobe component --
    // `tab-coordinator.test.ts` among them -- now reach this JSON through the
    // graph and fail at collection. `@lobehub/editor` is listed for the same
    // reason views lists it: it reaches the same JSON by its own path.
    server: {
      deps: {
        inline: ["@lobehub/ui", "@lobehub/editor"],
      },
    },
  },
});
