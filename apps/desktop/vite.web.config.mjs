import { fileURLToPath } from "node:url";
import { dirname, resolve } from "node:path";
import { env } from "node:process";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";
import { defineConfig } from "vite";

const desktopRoot = dirname(fileURLToPath(import.meta.url));
const port =
  Number(
    env.DESKTOP_RENDERER_PORT ?? env.VITE_PORT ?? env.FRONTEND_PORT,
  ) || 5173;

export default defineConfig({
  // Keep the browser renderer on the same env files as the Electron renderer.
  envDir: desktopRoot,
  // Worktree Make targets export NEXT_PUBLIC_API_URL/WS_URL because those are
  // also consumed by the Next.js client. They are public endpoint values, so
  // expose that convention to this Vite-only renderer as well.
  envPrefix: ["VITE_", "NEXT_PUBLIC_"],
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
