import { describe, expect, it, vi } from "vitest";
import { NextRequest } from "next/server";
import { PATCHBAY_LOCALE_HEADER } from "./lib/locale-routing";

vi.mock("@clerk/nextjs/server", async (importOriginal) => {
  const actual =
    await importOriginal<typeof import("@clerk/nextjs/server")>();
  return {
    ...actual,
    getAuth: async (request: NextRequest) => ({
      userId: request.cookies.has("patchbay_logged_in") ? "user-1" : null,
    }),
  };
});

import { proxy } from "./proxy";

function makeRequest(
  path: string,
  cookies: Record<string, string> = {},
  host = "app.patchbay.test",
) {
  const cookieHeader = Object.entries(cookies)
    .map(([key, value]) => `${key}=${value}`)
    .join("; ");

  return new NextRequest(`https://${host}${path}`, {
    headers: cookieHeader ? { cookie: cookieHeader } : undefined,
  });
}

async function redirectLocation(
  path: string,
  cookies: Record<string, string> = {},
  host?: string,
) {
  return (await proxy(makeRequest(path, cookies, host))).headers.get("location");
}

function restoreEnv(key: string, value: string | undefined) {
  if (value === undefined) delete process.env[key];
  else process.env[key] = value;
}

async function withoutRuntimeUpstreams(run: () => Promise<void>) {
  const previousRemoteApiUrl = process.env.REMOTE_API_URL;
  const previousDocsUrl = process.env.DOCS_URL;
  const previousPublicApiUrl = process.env.NEXT_PUBLIC_API_URL;
  const previousPort = process.env.PORT;
  delete process.env.REMOTE_API_URL;
  delete process.env.DOCS_URL;
  delete process.env.NEXT_PUBLIC_API_URL;
  process.env.PORT = "3000";

  try {
    await run();
  } finally {
    restoreEnv("REMOTE_API_URL", previousRemoteApiUrl);
    restoreEnv("DOCS_URL", previousDocsUrl);
    restoreEnv("NEXT_PUBLIC_API_URL", previousPublicApiUrl);
    restoreEnv("PORT", previousPort);
  }
}

describe("proxy legacy workspace route redirects", () => {
  const sessionCookies = {
    patchbay_logged_in: "1",
    last_workspace_slug: "acme",
  };

  it.each([
    ["issues", "/acme/issues"],
    ["projects", "/acme/projects"],
    ["agents", "/acme/agents"],
    ["squads", "/acme/squads"],
    ["inbox", "/acme/inbox"],
    ["my-issues", "/acme/my-issues"],
    ["autopilots", "/acme/autopilots"],
    ["runtimes", "/acme/runtimes"],
    ["skills", "/acme/skills"],
    ["settings", "/acme/settings"],
    ["usage", "/acme/usage"],
  ])(
    "redirects legacy /%s URLs through the last workspace slug",
    async (segment, expectedPath) => {
      expect(
        await redirectLocation(`/${segment}?tab=all`, sessionCookies),
      ).toBe(`https://app.patchbay.test${expectedPath}?tab=all`);
    },
  );

  it("preserves nested legacy paths and query strings", async () => {
    expect(
      await redirectLocation("/squads/squad-123?view=members", sessionCookies),
    ).toBe("https://app.patchbay.test/acme/squads/squad-123?view=members");
  });

  it("sends logged-out legacy URLs to login", async () => {
    expect(await redirectLocation("/usage?tab=billing")).toBe(
      "https://app.patchbay.test/login?tab=billing&redirect_url=%2Fusage",
    );
  });

  it("sends logged-in legacy URLs without a last workspace cookie to login", async () => {
    expect(
      await redirectLocation("/squads?view=members", { patchbay_logged_in: "1" }),
    ).toBe("https://app.patchbay.test/login");
  });

  it.each(["aspectlylabs.com", "aspectlylabs.com"])(
    "resolves a slugless session off the marketing host %s instead of stranding it",
    async (host) => {
      expect(
        await redirectLocation("/inbox", { patchbay_logged_in: "1" }, host),
      ).toBe(`https://${host}/login`);
    },
  );

  it("does not redirect workspace-scoped URLs whose first segment is already a slug", async () => {
    expect(await redirectLocation("/acme/squads", sessionCookies)).toBeNull();
  });

  it("redirects app-host root URLs to the last workspace", async () => {
    expect(await redirectLocation("/", sessionCookies)).toBe(
      "https://app.patchbay.test/acme/issues",
    );
  });

  it.each(["aspectlylabs.com", "aspectlylabs.com"])(
    "does not redirect public marketing root on %s",
    async (host) => {
      expect(await redirectLocation("/", sessionCookies, host)).toBeNull();
    },
  );

  it("still redirects explicit legacy app routes on the public marketing host", async () => {
    expect(
      await redirectLocation("/issues/ABC-123", sessionCookies, "aspectlylabs.com"),
    ).toBe("https://aspectlylabs.com/acme/issues/ABC-123");
  });
});

describe("proxy runtime upstream rewrites", () => {
  it("does not rewrite API requests when no runtime API origin is configured", async () => {
    await withoutRuntimeUpstreams(async () => {
      const res = await proxy(makeRequest("/api/config?x=1"));

      expect(res.status).toBe(200);
      expect(res.headers.get("x-middleware-rewrite")).toBeNull();
      expect(
        res.headers.get(`x-middleware-request-${PATCHBAY_LOCALE_HEADER}`),
      ).toBe("en");
    });
  });

  it("does not rewrite docs requests when no runtime docs origin is configured", async () => {
    await withoutRuntimeUpstreams(async () => {
      const res = await proxy(makeRequest("/docs/zh"));

      expect(res.status).toBe(200);
      expect(res.headers.get("x-middleware-rewrite")).toBeNull();
      expect(
        res.headers.get(`x-middleware-request-${PATCHBAY_LOCALE_HEADER}`),
      ).toBe("en");
    });
  });

  it.each([
    [
      "REMOTE_API_URL",
      "http://backend:8080",
      "/api/config?x=1",
      "http://backend:8080/api/config?x=1",
    ],
    [
      "DOCS_URL",
      "http://docs:4000",
      "/docs/zh/agents",
      "http://docs:4000/docs/zh/agents",
    ],
    [
      "REMOTE_API_URL",
      "http://backend:8080",
      "/ws",
      "http://backend:8080/ws",
    ],
  ])(
    "rewrites %s requests to the runtime origin",
    async (key, origin, path, expected) => {
      const previous = process.env[key];
      process.env[key] = origin;
      try {
        const res = await proxy(makeRequest(path));
        expect(res.status).toBe(200);
        expect(res.headers.get("x-middleware-rewrite")).toBe(expected);
      } finally {
        restoreEnv(key, previous);
      }
    },
  );

  it("does not rewrite frontend auth callback pages", async () => {
    const previous = process.env.REMOTE_API_URL;
    process.env.REMOTE_API_URL = "http://backend:8080";
    try {
      const res = await proxy(
        makeRequest("/auth/callback", { patchbay_logged_in: "1" }),
      );

      expect(res.status).toBe(200);
      expect(res.headers.get("x-middleware-rewrite")).toBeNull();
      expect(
        res.headers.get(`x-middleware-request-${PATCHBAY_LOCALE_HEADER}`),
      ).toBe("en");
    } finally {
      restoreEnv("REMOTE_API_URL", previous);
    }
  });
});

describe("proxy root and locale handling", () => {
  it("redirects logged-in root visits to the last workspace", async () => {
    const res = await proxy(
      makeRequest("/", {
        patchbay_logged_in: "1",
        last_workspace_slug: "acme",
      }),
    );

    expect(res.status).toBe(307);
    expect(res.headers.get("location")).toBe(
      "https://app.patchbay.test/acme/issues",
    );
  });

  it("forwards locale on login requests", async () => {
    const res = await proxy(
      makeRequest("/login", { "patchbay-locale": "zh-Hans" }),
    );

    expect(res.status).toBe(200);
    expect(res.headers.get("location")).toBeNull();
    expect(
      res.headers.get(`x-middleware-request-${PATCHBAY_LOCALE_HEADER}`),
    ).toBe("zh-Hans");
  });
});
