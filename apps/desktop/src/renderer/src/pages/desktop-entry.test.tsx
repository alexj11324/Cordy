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

function renderEntry(onGuestSession = vi.fn()) {
  return {
    onGuestSession,
    ...render(
      <I18nProvider locale="zh-Hans" resources={RESOURCES}>
        <DesktopEntryPage
          onEnableCloudMode={vi.fn().mockResolvedValue(undefined)}
          onGuestSession={onGuestSession}
        />
      </I18nProvider>,
    ),
  };
}

beforeEach(() => {
  vi.restoreAllMocks();
});

describe("DesktopEntryPage", () => {
  it("opens the localized Guest username dialog and creates a local session", async () => {
    const createGuestSession = vi.fn().mockResolvedValue({
      ok: true,
      session: { displayName: "Alice" } satisfies LocalGuestSession,
    });
    installDesktopAPI(createGuestSession);
    const { onGuestSession } = renderEntry();

    fireEvent.click(screen.getByRole("button", { name: "Guest" }));
    expect(
      await screen.findByText("请设置你的账号名"),
    ).toBeInTheDocument();
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
