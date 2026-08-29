import { cloudflare } from "@cloudflare/vite-plugin";
import mdx from "@mdx-js/rollup";
import { defineConfig } from "vite";
import vinext from "vinext";

export default defineConfig({
  plugins: [
    // Fumadocs generates imports with ?collection=...; compile those MDX
    // modules before Vinext/RSC's import analysis sees them.
    mdx({ enforce: "pre" }),
    vinext(),
    cloudflare({
      viteEnvironment: { name: "rsc", childEnvironments: ["ssr"] },
    }),
  ],
});
