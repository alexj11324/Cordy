import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { ReactNode } from "react";

const mocks = vi.hoisted(() => ({
  createGuestSession: vi.fn(),
  openExternal: vi.fn(),
}));

vi.mock("@patchbay/core/auth", () => ({
  useAuthStore: (selector: (state: { createGuestSession: typeof mocks.createGuestSession }) => unknown) =>
    selector({ createGuestSession: mocks.createGuestSession }),
}));

vi.mock("@patchbay/views/auth", () => ({
  LoginPage: ({ extra, onGoogleLogin }: { extra?: ReactNode; onGoogleLogin: () => void }) => (
    <div>
      <button type="button" onClick={onGoogleLogin}>Log in</button>
      {extra}
    </div>
  ),
}));

vi.mock("@patchbay/ui/components/common/patchbay-icon", () => ({
  PatchbayIcon: () => null,
}));

vi.mock("@patchbay/views/platform", () => ({
  DragStrip: () => null,
}));

vi.mock("@patchbay/views/i18n", () => ({
  useT: () => ({
    t: (select: (locale: { desktop: { entry: Record<string, string> } }) => string) =>
      select({
        desktop: {
          entry: {
            skip: "Continue without signing in",
            skipping: "Starting guest session…",
            guest_error: "Could not start a guest session",
          },
        },
      }),
  }),
}));

import { DesktopLoginPage } from "./login";

beforeEach(() => {
  mocks.createGuestSession.mockReset();
  mocks.openExternal.mockReset();
  mocks.createGuestSession.mockResolvedValue({ id: "guest-user", is_guest: true });
  Object.defineProperty(window, "desktopAPI", {
    configurable: true,
    value: {
      runtimeConfig: {
        ok: true,
        config: { appUrl: "https://accounts.aspectlylabs.com" },
      },
      openExternal: mocks.openExternal,
    },
  });
});

describe("DesktopLoginPage", () => {
  it("keeps formal login and offers a clear guest entry", () => {
    render(<DesktopLoginPage />);

    expect(screen.getByRole("button", { name: "Log in" })).toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: "Continue without signing in" }),
    ).toBeInTheDocument();
  });

  it("opens the configured public accounts login path for formal login", async () => {
    render(<DesktopLoginPage />);

    fireEvent.click(screen.getByRole("button", { name: "Log in" }));

    await waitFor(() => expect(mocks.openExternal).toHaveBeenCalledOnce());
    const [url] = mocks.openExternal.mock.calls[0] as [string];
    const parsed = new URL(url);
    expect(parsed.origin).toBe("https://accounts.aspectlylabs.com");
    expect(parsed.pathname).toBe("/login");
    expect(parsed.searchParams.get("platform")).toBe("desktop");
    expect(parsed.searchParams.get("code_challenge")).toHaveLength(43);
    expect(parsed.searchParams.get("state")).toHaveLength(43);
    expect(mocks.createGuestSession).not.toHaveBeenCalled();
  });

  it("starts a real guest session without opening the browser", async () => {
    render(<DesktopLoginPage />);

    fireEvent.click(screen.getByRole("button", { name: "Continue without signing in" }));

    await waitFor(() => expect(mocks.createGuestSession).toHaveBeenCalledOnce());
    expect(mocks.openExternal).not.toHaveBeenCalled();
  });
});
