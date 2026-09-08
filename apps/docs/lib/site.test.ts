import { beforeEach, describe, expect, it, vi } from "vitest";

const existingDocs = vi.hoisted(() => new Set<string>());

vi.mock("node:fs", () => ({
  existsSync: vi.fn((path: string) => {
    const normalized = path.replaceAll("\\", "/");
    return [...existingDocs].some((suffix) => normalized.endsWith(suffix));
  }),
}));

const pages = new Map<string, { url: string }>([
  ["en:", { url: "/" }],
  ["zh:", { url: "/zh" }],
  ["ko:", { url: "/ko" }],
  ["ja:", { url: "/ja" }],
  ["en:agents", { url: "/agents" }],
  ["zh:agents", { url: "/zh/agents" }],
  ["ko:agents", { url: "/ko/agents" }],
  ["ja:agents", { url: "/ja/agents" }],
]);

vi.mock("@/lib/source", () => ({
  source: {
    getPage: vi.fn((slugs: string[], lang: string) => {
      return pages.get(`${lang}:${slugs.join("/")}`) ?? null;
    }),
  },
}));

beforeEach(() => {
  existingDocs.clear();
  existingDocs.add("index.mdx");
  existingDocs.add("index.zh.mdx");
  existingDocs.add("agents.mdx");
  existingDocs.add("agents.zh.mdx");
});

describe("docsAlternates", () => {
  it("emits hreflang for the supported locales only", async () => {
    const { docsAlternates } = await import("./site");

    expect(docsAlternates(["agents"])).toEqual({
      canonical: "https://orvilo.aspectlylabs.com/docs/agents",
      languages: {
        en: "https://orvilo.aspectlylabs.com/docs/agents",
        zh: "https://orvilo.aspectlylabs.com/docs/zh/agents",
        "x-default": "https://orvilo.aspectlylabs.com/docs/agents",
      },
    });
  });

  it("never emits hreflang for an unsupported locale, even if an MDX file is present", async () => {
    existingDocs.add("agents.ko.mdx");
    existingDocs.add("agents.ja.mdx");
    const { docsAlternates } = await import("./site");

    const { languages } = docsAlternates(["agents"]);
    expect(languages).not.toHaveProperty("ko");
    expect(languages).not.toHaveProperty("ja");
  });

  it("keeps the locale root alternates limited to real localized MDX pages", async () => {
    const { docsAlternates } = await import("./site");

    expect(docsAlternates([])).toEqual({
      canonical: "https://orvilo.aspectlylabs.com/docs",
      languages: {
        en: "https://orvilo.aspectlylabs.com/docs",
        zh: "https://orvilo.aspectlylabs.com/docs/zh",
        "x-default": "https://orvilo.aspectlylabs.com/docs",
      },
    });
  });
});
