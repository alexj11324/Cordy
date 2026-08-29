import { fileURLToPath } from "node:url";
import { dirname, resolve } from "node:path";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";
import { defineConfig } from "vite";

const desktopRoot = dirname(fileURLToPath(import.meta.url));
const port =
  Number(
    process.env.DESKTOP_RENDERER_PORT ??
      process.env.VITE_PORT ??
      process.env.FRONTEND_PORT,
  ) || 5173;

export default defineConfig({
  // Keep the browser renderer on the same env files as the Electron renderer.
  envDir: desktopRoot,
  plugins: [react(), tailwindcss()],
  root: resolve(desktopRoot, "src/renderer"),
  base: "/",
  appType: "spa",
  resolve: {
    alias: {
      "@": resolve(desktopRoot, "src/renderer/src"),
    },
    dedupe: ["react", "react-dom", "@tanstack/react-query"],
  },
  server: {
    host: "127.0.0.1",
    port,
    strictPort: true,
    fs: {
      allow: [resolve(desktopRoot, "../..")],
    },
  },
});
