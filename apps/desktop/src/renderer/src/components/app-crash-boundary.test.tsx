import { describe, expect, it, vi, beforeEach, afterEach } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";

const captureException = vi.hoisted(() => vi.fn());
vi.mock("@patchbay/core/analytics", () => ({ captureException }));

// Marker stand-in so the structural assertion below tests the contract ("the
// drag strip is the first flex child") rather than DragStrip's own markup.
vi.mock("@patchbay/views/platform", () => ({
  DragStrip: () => <div data-testid="drag-strip" />,
}));

import { AppCrashBoundary } from "./app-crash-boundary";

function Boom(): never {
  throw new Error("useWorkspaceId: no workspace selected");
}

const getGuestMode = vi.fn<() => Promise<string>>();

beforeEach(() => {
  captureException.mockClear();
  getGuestMode.mockReset();
  getGuestMode.mockResolvedValue("cloud");
  Object.defineProperty(window, "desktopAPI", {
    configurable: true,
    writable: true,
    value: { getGuestMode },
  });
  // The boundary logs the captured error on purpose; keep the suite output
  // readable without hiding a genuinely unexpected console.error elsewhere.
  vi.spyOn(console, "error").mockImplementation(() => {});
});

afterEach(() => {
  vi.restoreAllMocks();
});

/**
 * MUL-6231 / #7021. The desktop renderer mounted <App /> with no boundary
 * above it, so one throw in the shell emptied the window and left force-quit
 * as the only way out.
 */
describe("AppCrashBoundary", () => {
  it("renders children when nothing throws", () => {
    render(
      <AppCrashBoundary>
        <div data-testid="app" />
      </AppCrashBoundary>,
    );

    expect(screen.queryByTestId("app")).not.toBeNull();
  });

  it("shows a recoverable fallback instead of blanking the window", () => {
    render(
      <AppCrashBoundary>
        <Boom />
      </AppCrashBoundary>,
    );

    const alert = screen.getByRole("alert");
    expect(alert).not.toBeNull();
    expect(alert.textContent).toContain("useWorkspaceId: no workspace selected");
    expect(screen.getByRole("button", { name: /reload/i })).not.toBeNull();
  });

  it("keeps the window draggable by mounting the drag strip as the first child", () => {
    // A full-window view outside the dashboard shell owns its own window
    // chrome. Without this the user loses the draggable top edge precisely
    // when the app is least usable — see CLAUDE.md Desktop Rules.
    const { container } = render(
      <AppCrashBoundary>
        <Boom />
      </AppCrashBoundary>,
    );

    const shell = container.firstElementChild;
    expect(shell).not.toBeNull();
    expect(shell).toHaveClass("flex", "flex-col");
    expect(shell?.firstElementChild).toHaveAttribute("data-testid", "drag-strip");
  });

  it("reports the crash through the exception pipeline, not as a plain event", async () => {
    // captureException routes into posthog's `$exception` path, the only one
    // initAnalytics' before_send hook redacts and de-dupes. A plain
    // captureEvent would ship the raw message and stack unredacted.
    render(
      <AppCrashBoundary>
        <Boom />
      </AppCrashBoundary>,
    );

    await waitFor(() => expect(captureException).toHaveBeenCalledTimes(1));
    expect(captureException).toHaveBeenCalledWith(
      expect.objectContaining({
        message: "useWorkspaceId: no workspace selected",
      }),
      { source: "desktop-renderer-boundary" },
    );
  });

  for (const mode of ["guest", "undecided"] as const) {
    it(`never reaches cloud analytics in ${mode} mode`, async () => {
      // Guest is local-only: a crash in the Guest shell must not ship the
      // user's error message and stack to PostHog. This fails the moment
      // anyone reinstates an unconditional captureException in the boundary.
      getGuestMode.mockResolvedValue(mode);

      render(
        <AppCrashBoundary>
          <Boom />
        </AppCrashBoundary>,
      );

      await waitFor(() => expect(getGuestMode).toHaveBeenCalled());
      // Let any deferred import settle; it must still not have captured.
      await Promise.resolve();
      await Promise.resolve();
      expect(captureException).not.toHaveBeenCalled();
      // The user still gets a recoverable window.
      expect(screen.getByRole("alert")).not.toBeNull();
    });
  }

  it("still renders the fallback when the mode gate itself fails", async () => {
    getGuestMode.mockRejectedValue(new Error("ipc unavailable"));

    render(
      <AppCrashBoundary>
        <Boom />
      </AppCrashBoundary>,
    );

    await waitFor(() => expect(getGuestMode).toHaveBeenCalled());
    expect(captureException).not.toHaveBeenCalled();
    expect(screen.getByRole("alert")).not.toBeNull();
  });
});
