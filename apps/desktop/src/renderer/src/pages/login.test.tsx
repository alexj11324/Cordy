// @vitest-environment jsdom

import {
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/react";
import "@testing-library/jest-dom/vitest";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { I18nProvider } from "@patchbay/core/i18n/react";
import { RESOURCES } from "@patchbay/views/locales";
import { DesktopLoginPage } from "./login";

const mocks = vi.hoisted(() => ({
  initiateDesktopAuthHandoff: vi.fn(),
  openExternal: vi.fn(),
  createDesktopLoginUrl: vi.fn(),
}));

vi.mock("@patchbay/core/api", () => ({
  api: { initiateDesktopAuthHandoff: mocks.initiateDesktopAuthHandoff },
}));

vi.mock("@patchbay/views/auth", () => ({
  LoginPage: () => <div data-testid="legacy-embedded-login" />,
}));

vi.mock("@patchbay/core/auth", () => ({
  useAuthStore: () => vi.fn(),
}));

vi.mock("@patchbay/ui/components/common/patchbay-icon", () => ({
  PatchbayIcon: () => <div data-testid="patchbay-icon" />,
}));

vi.mock("@patchbay/views/platform", () => ({
  DragStrip: () => null,
}));

vi.mock("./login-handoff", () => ({
  createDesktopLoginUrl: mocks.createDesktopLoginUrl,
}));

function renderPage(handoffFailed = false) {
  Object.defineProperty(window, "desktopAPI", {
    configurable: true,
    value: {
      runtimeConfig: {
        ok: true,
        config: {
          accountsUrl: "https://accounts.example",
          apiUrl: "https://api.aspectlylabs.com",
        },
      },
      openExternal: mocks.openExternal,
    },
  });

  return render(
    <I18nProvider locale="en" resources={RESOURCES}>
      <DesktopLoginPage handoffFailed={handoffFailed} />
    </I18nProvider>,
  );
}

beforeEach(() => {
  vi.restoreAllMocks();
  document.documentElement.lang = "en";
  mocks.createDesktopLoginUrl.mockImplementation(
    async (
      _accountsUrl: string,
      register: (state: string, challenge: string) => Promise<unknown>,
    ) => {
      await register("state-1", "challenge-1");
      return "https://accounts.example/login?state=state-1";
    },
  );
  mocks.initiateDesktopAuthHandoff.mockResolvedValue({ registered: true });
  mocks.openExternal.mockResolvedValue(undefined);
});

afterEach(() => {
  cleanup();
});

describe("DesktopLoginPage", () => {
  it("keeps the Clerk form out of Desktop", () => {
    renderPage();

    expect(screen.getByTestId("desktop-login-pending")).toHaveClass(
      "bg-zinc-950",
    );
    expect(
      screen.queryByTestId("authentication-form-panel"),
    ).not.toBeInTheDocument();
    expect(screen.queryByText("Continue with Google")).not.toBeInTheDocument();
  });

  it("can reopen the Accounts login page through a fresh PKCE handoff", async () => {
    renderPage();

    fireEvent.click(screen.getByRole("button", { name: "Open sign-in" }));

    await waitFor(() => {
      expect(mocks.initiateDesktopAuthHandoff).toHaveBeenCalledWith(
        "state-1",
        "challenge-1",
      );
    });
    expect(mocks.createDesktopLoginUrl).toHaveBeenCalledWith(
      "https://accounts.example",
      expect.any(Function),
      { sessionApiUrl: undefined, locale: "en" },
    );
    expect(mocks.openExternal).toHaveBeenCalledWith(
      "https://accounts.example/login?state=state-1",
    );
  });
});

it("shows a terminal callback failure instead of silently waiting", () => {
  renderPage(true);
  expect(screen.getByRole("alert")).toHaveTextContent("Sign-in could not be completed");
  expect(screen.getByRole("button", { name: "Open sign-in" })).toBeEnabled();
});
