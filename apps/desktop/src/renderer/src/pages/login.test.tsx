import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

const mocks = vi.hoisted(() => ({
  createGuestSession: vi.fn(),
  openExternal: vi.fn(),
}));

vi.mock("@patchbay/core/auth", () => ({
  useAuthStore: (selector: (state: { createGuestSession: typeof mocks.createGuestSession }) => unknown) =>
    selector({ createGuestSession: mocks.createGuestSession }),
}));

vi.mock("@patchbay/views/i18n", () => ({
  useT: () => ({
    t: (select: (messages: { desktop: { entry: Record<string, string> } }) => unknown) =>
      select({
        desktop: {
          entry: {
            title: "Welcome to Patchbay",
            description: "Use a real workspace.",
            login: "Log in",
            skip: "Continue as guest",
            skipping: "Starting guest session…",
            login_error: "Could not open login.",
            guest_error: "Could not start guest session.",
          },
        },
      }),
  }),
}));

vi.mock("@patchbay/ui/components/common/patchbay-icon", () => ({
  PatchbayIcon: () => <span aria-hidden="true" data-testid="patchbay-icon" />,
}));

vi.mock("@patchbay/views/platform", () => ({
  DragStrip: () => <div data-testid="drag-strip" />,
}));

import { DesktopLoginPage } from "./login";

describe("DesktopLoginPage", () => {
  beforeEach(() => {
    mocks.createGuestSession.mockReset().mockResolvedValue({
      id: "guest-user",
      is_guest: true,
    });
    mocks.openExternal.mockReset().mockResolvedValue(undefined);
    Object.defineProperty(window, "desktopAPI", {
      configurable: true,
      value: {
        runtimeConfig: {
          ok: true,
          config: { appUrl: "https://app.example.test" },
        },
        openExternal: mocks.openExternal,
      },
    });
  });

  it("renders only the app-owned login and guest actions", () => {
    render(<DesktopLoginPage />);

    expect(screen.getByRole("button", { name: "Log in" })).toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: "Continue as guest" }),
    ).toBeInTheDocument();
    expect(screen.queryByRole("textbox")).not.toBeInTheDocument();
    expect(screen.queryByText(/Clerk|Connect/i)).not.toBeInTheDocument();
  });

  it("opens the web login only from the login action", () => {
    render(<DesktopLoginPage />);

    fireEvent.click(screen.getByRole("button", { name: "Log in" }));

    expect(mocks.openExternal).toHaveBeenCalledWith(
      "https://app.example.test/login?platform=desktop",
    );
    expect(mocks.createGuestSession).not.toHaveBeenCalled();
  });

  it("creates a real guest session without opening a browser", async () => {
    render(<DesktopLoginPage />);

    fireEvent.click(screen.getByRole("button", { name: "Continue as guest" }));

    await waitFor(() => expect(mocks.createGuestSession).toHaveBeenCalledOnce());
    expect(mocks.openExternal).not.toHaveBeenCalled();
  });
});
