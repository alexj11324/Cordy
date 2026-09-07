import type { MetadataRoute } from "next";

export default function robots(): MetadataRoute.Robots {
  return {
    rules: {
      userAgent: "*",
      allow: ["/docs", "/docs/"],
      disallow: "/",
    },
    sitemap: "https://orvilo.aspectlylabs.com/docs/sitemap.xml",
  };
}
