import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import "@testing-library/jest-dom/vitest";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { I18nProvider } from "@patchbay/core/i18n/react";
import { RESOURCES } from "@patchbay/views/locales";
import type { LocalGuestSession } from "../../../shared/local-guest";
import { DesktopEntryPage } from "./desktop-entry";

function installDesktopAPI(createGuestSession: ReturnType<typeof vi.fn>) {
  Object.defineProperty(window, "desktopAPI", {
    configurable: true,
    value: { createGuestSession },
  });
}

function renderEntry({
  onSignIn = vi.fn<() => Promise<void>>().mockResolvedValue(undefined),
  onGuestSession = vi.fn<(session: LocalGuestSession) => void>(),
}: {
  onSignIn?: () => Promise<void>;
  onGuestSession?: (session: LocalGuestSession) => void;
} = {}) {
  return {
    onSignIn,
    onGuestSession,
    ...render(
      <I18nProvider locale="zh-Hans" resources={RESOURCES}>
        <DesktopEntryPage onSignIn={onSignIn} onGuestSession={onGuestSession} />
      </I18nProvider>,
    ),
  };
}

beforeEach(() => {
  vi.restoreAllMocks();
});

describe("DesktopEntryPage", () => {
  it("shows the black welcome surface with Sign in and Guest side by side", () => {
    installDesktopAPI(vi.fn());
    renderEntry();

    expect(screen.getByTestId("desktop-entry")).toHaveClass("bg-zinc-950");
    expect(screen.getByTestId("desktop-entry-brand")).toHaveTextContent(
      "Patchbay",
    );
    expect(screen.getByTestId("desktop-entry-actions")).toContainElement(
      screen.getByRole("button", { name: "登录" }),
    );
    expect(screen.getByTestId("desktop-entry-actions")).toContainElement(
      screen.getByRole("button", { name: "Guest" }),
    );
  });

  it("opens browser sign-in from the welcome surface", async () => {
    installDesktopAPI(vi.fn());
    const onSignIn = vi.fn().mockResolvedValue(undefined);
    renderEntry({ onSignIn });

    fireEvent.click(screen.getByRole("button", { name: "登录" }));

    await waitFor(() => expect(onSignIn).toHaveBeenCalledOnce());
  });

  it("keeps the sign-in controls visually stable while opening", async () => {
    installDesktopAPI(vi.fn());
    let finish!: () => void;
    const onSignIn = vi.fn(
      () =>
        new Promise<void>((resolve) => {
          finish = resolve;
        }),
    );
    renderEntry({ onSignIn });

    const button = screen.getByRole("button", { name: "登录" });
    fireEvent.click(button);

    await waitFor(() => expect(button).toHaveAttribute("aria-busy", "true"));
    expect(button).toHaveTextContent("登录");
    expect(button).toHaveClass(
      "disabled:opacity-100",
      "transition-none",
      "active:not-aria-[haspopup]:translate-y-0",
    );
    expect(screen.getByTestId("desktop-entry-feedback")).toHaveClass(
      "min-h-5",
    );
    finish();
  });

  it("opens the localized Guest username dialog and creates a local session", async () => {
    const createGuestSession = vi.fn().mockResolvedValue({
      ok: true,
      session: { displayName: "Alice" } satisfies LocalGuestSession,
    });
    installDesktopAPI(createGuestSession);
    const { onGuestSession } = renderEntry();

    fireEvent.click(screen.getByRole("button", { name: "Guest" }));
    expect(await screen.findByText("请设置你的账号名")).toBeInTheDocument();
    fireEvent.change(screen.getByLabelText("账号名"), {
      target: { value: "Alice" },
    });
    fireEvent.click(screen.getByRole("button", { name: "以 Guest 身份继续" }));

    await waitFor(() => {
      expect(createGuestSession).toHaveBeenCalledWith("Alice");
      expect(onGuestSession).toHaveBeenCalledWith({ displayName: "Alice" });
    });
  });

  it("keeps the Guest dialog open when the main process rejects the name", async () => {
    const createGuestSession = vi.fn().mockResolvedValue({
      ok: false,
      reason: "invalid_name",
    });
    installDesktopAPI(createGuestSession);
    renderEntry();

    fireEvent.click(screen.getByRole("button", { name: "Guest" }));
    fireEvent.change(screen.getByLabelText("账号名"), {
      target: { value: "bad\nname" },
    });
    fireEvent.click(screen.getByRole("button", { name: "以 Guest 身份继续" }));

    expect(await screen.findByRole("alert")).toHaveTextContent(
      "请输入 1–64 个字符，且不要包含控制字符。",
    );
    expect(screen.getByText("请设置你的账号名")).toBeInTheDocument();
  });
});
