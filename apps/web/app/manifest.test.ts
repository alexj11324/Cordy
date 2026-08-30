import { describe, expect, it, vi } from "vitest";
import { NextRequest } from "next/server";

vi.mock("@clerk/nextjs/server", async (importOriginal) => {
  const actual =
    await importOriginal<typeof import("@clerk/nextjs/server")>();
  type TestClerkMiddlewareHandler = (
    auth: () => Promise<{ userId: string | null }>,
    request: NextRequest,
    event: unknown,
  ) => Response | null | undefined | Promise<Response | null | undefined>;

  return {
    ...actual,
    clerkMiddleware: (handler: TestClerkMiddlewareHandler) =>
      async (request: NextRequest) =>
        handler(
          async () => ({
            userId: request.cookies.has("patchbay_logged_in")
              ? "user-1"
              : null,
          }),
          request,
          undefined as never,
        ),
    getAuth: async (request: NextRequest) => ({
      userId: request.cookies.has("patchbay_logged_in") ? "user-1" : null,
    }),
  };
});

import { proxy } from "../proxy";
import manifest, { PWA_START_URL } from "./manifest";

async function runProxy(request: NextRequest) {
  const response = await proxy(request);
  if (!response) throw new Error("proxy returned no response");
  return response;
}

async function launch(
  cookies: Record<string, string>,
  host = "www.patchbay.ai",
) {
  const cookieHeader = Object.entries(cookies)
    .map(([key, value]) => `${key}=${value}`)
    .join("; ");

  const response = await runProxy(
    new NextRequest(`https://${host}${PWA_START_URL}`, {
      headers: cookieHeader ? { cookie: cookieHeader } : undefined,
    }),
  );
  return response.headers.get("location");
}

describe("web app manifest", () => {
  it("declares the fields a browser needs to offer installation", () => {
    const value = manifest();

    expect(value.display).toBe("standalone");
    expect(value.scope).toBe("/");
    expect(value.name).toBeTruthy();
    expect(value.short_name).toBeTruthy();
  });

  it("ships both icon sizes Chrome requires, plus a maskable one", () => {
    const icons = manifest().icons ?? [];
    const sizesFor = (purpose: string) =>
      icons.filter((icon) => icon.purpose === purpose).map((icon) => icon.sizes);

    expect(sizesFor("any")).toEqual(
      expect.arrayContaining(["192x192", "512x512"]),
    );
    expect(sizesFor("maskable")).toContain("512x512");
    for (const icon of icons) expect(icon.src.startsWith("/icons/")).toBe(true);
  });

  it("does not launch at the root path", () => {
    expect(manifest().start_url).not.toBe("/");
  });

  it("launches into the last workspace for a signed-in session", async () => {
    expect(
      await launch({ patchbay_logged_in: "1", last_workspace_slug: "acme" }),
    ).toContain("/acme/inbox");
  });

  it("launches into login when there is no session", async () => {
    expect(await launch({})).toContain("/login");
  });

  it("never launches onto the marketing site for a session with no known workspace", async () => {
    const target = await launch({ patchbay_logged_in: "1" });

    expect(target).toContain("/login");
    expect(new URL(target ?? "", "https://www.patchbay.ai").pathname).not.toBe(
      "/",
    );
  });

  it("points every shortcut at a path that resolves the same way", async () => {
    const shortcuts = manifest().shortcuts ?? [];

    expect(shortcuts.length).toBeGreaterThan(0);
    for (const shortcut of shortcuts) {
      const resolve = async (cookie: string) => {
        const response = await runProxy(
          new NextRequest(`https://www.patchbay.ai${shortcut.url}`, {
            headers: { cookie },
          }),
        );
        return response.headers.get("location");
      };

      expect(
        await resolve("patchbay_logged_in=1; last_workspace_slug=acme"),
      ).toContain(`/acme${shortcut.url}`);
      expect(await resolve("patchbay_logged_in=1")).toContain("/login");
      expect(await resolve("")).toContain("/login");
    }
  });
});
