import { createMDX } from "fumadocs-mdx/next";

const withMDX = createMDX();

/** @type {import('next').NextConfig} */
const config = {
  output: "standalone",
  reactStrictMode: true,
  basePath: "/docs",
  // Visiting http://host/ (outside basePath) would otherwise 404 — redirect
  // to the docs root. basePath: false makes the source and destination
  // literal (not re-prefixed with `/docs`), so the redirect runs before
  // basePath routing kicks in.
  async redirects() {
    return [
      {
        source: "/",
        destination: "/docs",
        basePath: false,
        permanent: false,
      },
      {
        source: "/zh/getting-started/cloud-quickstart",
        destination: "/zh/cloud-quickstart",
        permanent: true,
      },
      {
        source: "/zh/getting-started/self-hosting",
        destination: "/zh/self-host-quickstart",
        permanent: true,
      },
      {
        source: "/zh/guides/quickstart",
        destination: "/zh/cloud-quickstart",
        permanent: true,
      },
      {
        source: "/zh/guides/agents",
        destination: "/zh/agents",
        permanent: true,
      },
      {
        source: "/zh/cli/installation",
        destination: "/zh/cli",
        permanent: true,
      },
      {
        source: "/zh/cli/reference",
        destination: "/zh/cli",
        permanent: true,
      },
      {
        source: "/how-cordy-works", // legacy-brand-compat
        destination: "/how-patchbay-works",
        permanent: true,
      },
      {
        source: "/zh/how-cordy-works", // legacy-brand-compat
        destination: "/zh/how-patchbay-works",
        permanent: true,
      },
      {
        source: "/ja/how-cordy-works", // legacy-brand-compat
        destination: "/ja/how-patchbay-works",
        permanent: true,
      },
      {
        source: "/ko/how-cordy-works", // legacy-brand-compat
        destination: "/ko/how-patchbay-works",
        permanent: true,
      },
    ];
  },
};

export default withMDX(config);
