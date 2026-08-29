import { cloudflare } from "@cloudflare/vite-plugin";
import mdx from "@mdx-js/rollup";
import { defineConfig } from "vite";
import vinext from "vinext";

const mdxPlugin = Object.assign(mdx(), { enforce: "pre" as const });

export default defineConfig({
  plugins: [
    // Fumadocs generates imports with ?collection=...; compile those MDX
    // modules before Vinext/RSC's import analysis sees them.
    mdxPlugin,
    vinext(),
    cloudflare({
      viteEnvironment: { name: "rsc", childEnvironments: ["ssr"] },
    }),
  ],
});
