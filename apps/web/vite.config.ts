import { cloudflare } from "@cloudflare/vite-plugin";
import { cdnAdapter } from "@vinext/cloudflare/cache/cdn-adapter";
import mdx from "@mdx-js/rollup";
import { defineConfig } from "vite";
import vinext from "vinext";

const mdxPlugin = Object.assign(mdx(), { enforce: "pre" as const });

export default defineConfig({
  plugins: [
    // Fumadocs generates imports with ?collection=...; compile those MDX
    // modules before Vinext/RSC's import analysis sees them.
    mdxPlugin,
    vinext({ cache: { cdn: cdnAdapter() } }),
    cloudflare({
      viteEnvironment: { name: "rsc", childEnvironments: ["ssr"] },
    }),
  ],
});
