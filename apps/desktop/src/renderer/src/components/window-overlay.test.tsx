// @vitest-environment jsdom
import { beforeEach, describe, expect, it, vi } from "vitest";
import { act, render, screen, waitFor } from "@testing-library/react";
import { useWindowOverlayStore } from "@/stores/window-overlay-store";

vi.mock("./desktop-settings-page", () => ({
  DesktopSettingsPage: ({ onBack }: { onBack?: () => void }) => (
    <button data-settings-initial-focus type="button" onClick={onBack}>
      Back to app
    </button>
  ),
}));

vi.mock("@patchbay/views/invite", () => ({ InvitePage: () => null }));
vi.mock("@patchbay/views/invitations", () => ({ InvitationsPage: () => null }));
vi.mock("@patchbay/views/onboarding", () => ({ OnboardingFlow: () => null }));
vi.mock("@patchbay/views/navigation", () => ({
  useNavigation: () => ({ push: vi.fn() }),
}));
vi.mock("../platform/use-local-runtimes-pending", () => ({
  useLocalRuntimesPending: () => false,
}));

import { WindowOverlay } from "./window-overlay";

beforeEach(() => {
  useWindowOverlayStore.getState().close();
});

describe("SettingsWindow focus", () => {
  it("moves focus into Settings and restores the prior control on close", async () => {
    render(
      <>
        <button type="button">Underlying action</button>
        <WindowOverlay />
      </>,
    );
    const underlying = screen.getByRole("button", {
      name: "Underlying action",
    });
    underlying.focus();

    act(() => {
      useWindowOverlayStore.getState().open({
        type: "settings",
        path: "/acme/settings",
      });
    });
    expect(screen.getByRole("button", { name: "Back to app" })).toHaveFocus();

    act(() => useWindowOverlayStore.getState().close());
    await waitFor(() => expect(underlying).toHaveFocus());
  });
});
