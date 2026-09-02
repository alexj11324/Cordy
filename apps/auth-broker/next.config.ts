import type { NextConfig } from "next";
import { resolve } from "node:path";

const nextConfig: NextConfig = {
  reactStrictMode: true,
  outputFileTracingRoot: resolve(__dirname, "../.."),
  ...(process.env.STANDALONE === "true" ? { output: "standalone" as const } : {}),
  async headers() { return [{ source: "/:path*", headers: [
    { key: "Cache-Control", value: "no-store" }, { key: "Referrer-Policy", value: "no-referrer" },
    { key: "X-Content-Type-Options", value: "nosniff" }, { key: "X-Frame-Options", value: "DENY" },
    { key: "Permissions-Policy", value: "camera=(), microphone=(), geolocation=()" },
    { key: "X-Patchbay-Build", value: process.env.NEXT_PUBLIC_APP_VERSION || "dev" }
  ] }]; }
};
export default nextConfig;
