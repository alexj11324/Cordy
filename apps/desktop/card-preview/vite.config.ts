import { fileURLToPath } from "node:url";
import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";

// Separate from electron-vite and Next: never a route in a shipped application.
export default defineConfig(({ command }) => {
  if (command !== "serve")
    throw new Error("Card preview is development-only; it cannot be built.");
  return {
    root: fileURLToPath(new URL(".", import.meta.url)),
    envDir: false,
    plugins: [react(), tailwindcss()],
    server: {
      host: "127.0.0.1",
      port: 5188,
      strictPort: true,
      // Deliberately manual refresh: connect-src none also blocks HMR sockets.
      hmr: false,
      headers: {
        "Content-Security-Policy":
          "default-src 'self'; script-src 'self' 'unsafe-inline'; style-src 'self' 'unsafe-inline'; font-src 'self' data:; img-src 'self' data:; connect-src 'none'; form-action 'none'; frame-src 'none'; object-src 'none'; base-uri 'none'",
      },
    },
  };
});
