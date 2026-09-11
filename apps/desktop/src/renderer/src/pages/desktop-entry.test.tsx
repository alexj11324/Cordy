import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import "@testing-library/jest-dom/vitest";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { I18nProvider } from "@orvilo/core/i18n/react";
import { RESOURCES } from "@orvilo/views/locales";
import { DesktopEntryPage } from "./desktop-entry";

function installDesktopAPI(createGuestSession: ReturnType<typeof vi.fn>) {
  Object.defineProperty(window, "desktopAPI", {
    configurable: true,
    value: { createGuestSession },
  });
}

function renderEntry({
  onSignIn = vi.fn<() => Promise<void>>().mockResolvedValue(undefined),
  onGuestSession = vi.fn<() => Promise<void>>().mockResolvedValue(undefined),
  onResetGuest,
}: {
  onSignIn?: () => Promise<void>;
  onGuestSession?: () => Promise<void>;
  onResetGuest?: () => Promise<void>;
} = {}) {
  return {
    onSignIn,
    onGuestSession,
    ...render(
      <I18nProvider locale="zh-Hans" resources={RESOURCES}>
        <DesktopEntryPage onSignIn={onSignIn} onGuestSession={onGuestSession} onResetGuest={onResetGuest} />
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

    expect(screen.getByTestId("desktop-entry")).toHaveClass("dark", "bg-background");
    expect(screen.getByTestId("desktop-entry-brand")).toHaveTextContent(
      "Orvilo",
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

  it("starts Guest without a username dialog or local-only session", async () => {
    const oldLocalCreate = vi.fn();
    installDesktopAPI(oldLocalCreate);
    const { onGuestSession } = renderEntry();
    fireEvent.click(screen.getByRole("button", { name: "Guest" }));
    await waitFor(() => expect(onGuestSession).toHaveBeenCalledOnce());
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    expect(oldLocalCreate).not.toHaveBeenCalled();
  });

  it("shows Guest failure and allows another attempt", async () => {
    const onGuestSession = vi.fn().mockRejectedValue(new Error("offline"));
    renderEntry({ onGuestSession });
    fireEvent.click(screen.getByRole("button", { name: "Guest" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("无法开始 Guest 会话");
    expect(screen.getByRole("button", { name: "Guest" })).toBeEnabled();
  });
  it("keeps recovery for a damaged legacy marker on the regular entry page", async () => {
    const onResetGuest = vi.fn().mockResolvedValue(undefined);
    renderEntry({ onResetGuest });
    fireEvent.click(screen.getByRole("button", { name: "重置 Guest 会话" }));
    await waitFor(() => expect(onResetGuest).toHaveBeenCalledOnce());
    expect(screen.getByRole("button", { name: "登录" })).toBeInTheDocument();
    expect(screen.queryByText("本地工作区运行")).not.toBeInTheDocument();
  });

});
