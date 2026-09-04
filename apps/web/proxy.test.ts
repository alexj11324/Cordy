import { describe, expect, it, vi } from "vitest";
import { NextRequest } from "next/server";
import { PATCHBAY_LOCALE_HEADER } from "./lib/locale-routing";

vi.mock("@clerk/nextjs/server", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@clerk/nextjs/server")>();
  type TestClerkMiddlewareHandler = (
    auth: () => Promise<{ userId: string | null }>,
    request: NextRequest,
    event: unknown,
  ) => Response | null | undefined | Promise<Response | null | undefined>;

  return {
    ...actual,
    clerkMiddleware:
      (handler: TestClerkMiddlewareHandler) => async (request: NextRequest) =>
        handler(
          async () => ({
            userId: request.cookies.has("patchbay_logged_in") ? "user-1" : null,
          }),
          request,
          undefined as never,
        ),
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

async function runProxy(request: NextRequest) {
  const response = await proxy(request);
  if (!response) throw new Error("proxy returned no response");
  return response;
}

async function redirectLocation(
  path: string,
  cookies: Record<string, string> = {},
  host?: string,
) {
  return (await runProxy(makeRequest(path, cookies, host))).headers.get(
    "location",
  );
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
    ["teams", "/acme/teams"],
    ["inbox", "/acme/inbox"],
    ["my-issues", "/acme/my-issues"],
    ["automations", "/acme/automations"],
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
      await redirectLocation("/teams/team-123?view=members", sessionCookies),
    ).toBe("https://app.patchbay.test/acme/teams/team-123?view=members");
  });

  it("sends logged-out legacy URLs to login", async () => {
    expect(await redirectLocation("/usage?tab=billing")).toBe(
      "https://app.patchbay.test/login?redirect_url=%2Fusage%3Ftab%3Dbilling",
    );
  });

  it("sends logged-in legacy URLs without a last workspace cookie to login", async () => {
    expect(
      await redirectLocation("/teams?view=members", {
        patchbay_logged_in: "1",
      }),
    ).toBe("https://app.patchbay.test/login");
  });

  it.each(["patchbay.aspectlylabs.com"])(
    "resolves a slugless session on the production app host %s instead of stranding it",
    async (host) => {
      expect(
        await redirectLocation("/inbox", { patchbay_logged_in: "1" }, host),
      ).toBe(`https://${host}/login`);
    },
  );

  it("does not redirect workspace-scoped URLs whose first segment is already a slug", async () => {
    expect(await redirectLocation("/acme/teams", sessionCookies)).toBeNull();
  });

  it.each(["app.patchbay.test", "patchbay.aspectlylabs.com"])(
    "redirects root URLs to the last workspace on %s",
    async (host) => {
      expect(await redirectLocation("/", sessionCookies, host)).toBe(
        `https://${host}/acme/issues`,
      );
    },
  );

  it("redirects explicit legacy app routes on every host", async () => {
    expect(
      await redirectLocation(
        "/issues/ABC-123",
        sessionCookies,
        "patchbay.aspectlylabs.com",
      ),
    ).toBe("https://patchbay.aspectlylabs.com/acme/issues/ABC-123");
  });
});

describe("proxy runtime upstream rewrites", () => {
  it("does not rewrite API requests when no runtime API origin is configured", async () => {
    await withoutRuntimeUpstreams(async () => {
      const res = await runProxy(makeRequest("/api/config?x=1"));

      expect(res.status).toBe(200);
      expect(res.headers.get("x-middleware-rewrite")).toBeNull();
      expect(
        res.headers.get(`x-middleware-request-${PATCHBAY_LOCALE_HEADER}`),
      ).toBe("en");
    });
  });

  it("does not rewrite docs requests when no runtime docs origin is configured", async () => {
    await withoutRuntimeUpstreams(async () => {
      const res = await runProxy(makeRequest("/docs/zh"));

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
    ["REMOTE_API_URL", "http://backend:8080", "/ws", "http://backend:8080/ws"],
  ])(
    "rewrites %s requests to the runtime origin",
    async (key, origin, path, expected) => {
      const previous = process.env[key];
      process.env[key] = origin;
      try {
        const res = await runProxy(makeRequest(path));
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
      const res = await runProxy(
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
    const res = await runProxy(
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
    const res = await runProxy(
      makeRequest("/login", { "patchbay-locale": "zh-Hans" }),
    );

    expect(res.status).toBe(200);
    expect(res.headers.get("location")).toBeNull();
    expect(
      res.headers.get(`x-middleware-request-${PATCHBAY_LOCALE_HEADER}`),
    ).toBe("zh-Hans");
  });

  it("leaves the legacy frontend auth callback public", async () => {
    expect(await redirectLocation("/auth/callback")).toBeNull();
  });

  it("sends Desktop Google broker routes to hosted Accounts", async () => {
    const previous = process.env.PATCHBAY_AUTH_BROKER_ORIGIN;
    process.env.PATCHBAY_AUTH_BROKER_ORIGIN =
      "https://accounts.aspectlylabs.com";
    try {
      expect(
        await redirectLocation(
          `/oauth/google?platform=desktop&code_challenge=${"a".repeat(43)}&state=${"b".repeat(43)}`,
        ),
      ).toBe(
        `https://accounts.aspectlylabs.com/oauth/google?platform=desktop&code_challenge=${"a".repeat(43)}&state=${"b".repeat(43)}`,
      );
      expect(
        await redirectLocation(
          `/oauth/google/callback?platform=desktop&code_challenge=${"a".repeat(43)}&state=${"b".repeat(43)}`,
        ),
      ).toBe(
        `https://accounts.aspectlylabs.com/oauth/google/callback?platform=desktop&code_challenge=${"a".repeat(43)}&state=${"b".repeat(43)}`,
      );
    } finally {
      restoreEnv("PATCHBAY_AUTH_BROKER_ORIGIN", previous);
    }
  });

  it("sends Desktop login off the product web origin onto hosted Accounts", async () => {
    const previous = process.env.PATCHBAY_AUTH_BROKER_ORIGIN;
    process.env.PATCHBAY_AUTH_BROKER_ORIGIN =
      "https://accounts.aspectlylabs.com";
    const challenge = "a".repeat(43);
    const state = "b".repeat(43);
    try {
      expect(
        await redirectLocation(
          `/login?platform=desktop&code_challenge=${challenge}&state=${state}&session_api=http%3A%2F%2Flocalhost%3A8080`,
        ),
      ).toBe(
        `https://accounts.aspectlylabs.com/login?platform=desktop&code_challenge=${challenge}&state=${state}&session_api=http%3A%2F%2Flocalhost%3A8080`,
      );
    } finally {
      restoreEnv("PATCHBAY_AUTH_BROKER_ORIGIN", previous);
    }
  });

  it("does not send self-host Desktop login to production Accounts", async () => {
    const previousBroker = process.env.PATCHBAY_AUTH_BROKER_ORIGIN;
    const previousAccounts = process.env.NEXT_PUBLIC_ACCOUNTS_URL;
    delete process.env.PATCHBAY_AUTH_BROKER_ORIGIN;
    delete process.env.NEXT_PUBLIC_ACCOUNTS_URL;
    try {
      expect(
        await redirectLocation(
          `/login?platform=desktop&code_challenge=${"a".repeat(43)}&state=${"b".repeat(43)}`,
        ),
      ).toBeNull();
    } finally {
      restoreEnv("PATCHBAY_AUTH_BROKER_ORIGIN", previousBroker);
      restoreEnv("NEXT_PUBLIC_ACCOUNTS_URL", previousAccounts);
    }
  });

  it("does not send ordinary web login to Accounts", async () => {
    expect(await redirectLocation("/login")).toBeNull();
  });
});
