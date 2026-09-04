import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";

const {
  mockSearchParams,
  mockGoogleLogin,
  mockCompleteDesktopAuthHandoff,
} = vi.hoisted(() => ({
  mockSearchParams: new URLSearchParams(),
  mockGoogleLogin: vi.fn(),
  mockCompleteDesktopAuthHandoff: vi.fn(),
}));

vi.mock("next/navigation", () => ({
  useSearchParams: () => mockSearchParams,
}));

vi.mock("@patchbay/core/api", () => ({
  api: {
    googleLogin: mockGoogleLogin,
    completeDesktopAuthHandoff: mockCompleteDesktopAuthHandoff,
  },
}));

import CallbackPage from "./page";

describe("CallbackPage", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    // Reset the source-backfill dismiss counter so a test that writes
    // it doesn't leak state into the next test (and the next test
    // doesn't inherit a cap-reached state from a previous run).
    for (let i = window.localStorage.length - 1; i >= 0; i--) {
      const k = window.localStorage.key(i);
      if (k && k.startsWith("patchbay.source_backfill.dismiss.")) {
        window.localStorage.removeItem(k);
      }
    }
    // Snapshot keys before deleting — forEach + delete skips entries because
    // the iteration index advances while the underlying list shrinks.
    Array.from(mockSearchParams.keys()).forEach((k) =>
      mockSearchParams.delete(k),
    );
    mockSearchParams.set("code", "test-code");
    mockGoogleLogin.mockResolvedValue({ token: "unused-web-token" });
    mockCompleteDesktopAuthHandoff.mockResolvedValue({
      callback_protocol: "patchbay",
      code: "desktop-code",
      state: "desktop-state",
    });
  });

  it("rejects a state-less callback without invoking the Web Google exchange", async () => {
    render(<CallbackPage />);

    await waitFor(() => {
      expect(screen.getByText("Unsupported login callback")).toBeInTheDocument();
    });
    expect(mockGoogleLogin).not.toHaveBeenCalled();
  });

  it("surfaces an OAuth error callback before checking for a code", async () => {
    mockSearchParams.delete("code");
    mockSearchParams.set("error", "access_denied");

    render(<CallbackPage />);

    await waitFor(() => {
      expect(screen.getByText("Access denied")).toBeInTheDocument();
    });
    expect(mockGoogleLogin).not.toHaveBeenCalled();
  });

  it("redirects to CLI callback with token when state contains valid cli_callback", async () => {
    const hrefSetter = vi.fn();
    const originalLocation = window.location;
    Object.defineProperty(window, "location", {
      configurable: true,
      writable: true,
      value: { ...originalLocation, set href(value: string) { hrefSetter(value); } },
    });

    try {
      mockSearchParams.set(
        "state",
        "cli_callback:http://127.0.0.1:46233/callback,cli_state:abc123",
      );
      mockGoogleLogin.mockResolvedValue({ token: "cli-jwt-token" });

      render(<CallbackPage />);

      await waitFor(() => {
        expect(mockGoogleLogin).toHaveBeenCalledWith(
          "test-code",
          expect.stringContaining("/auth/callback"),
        );
      });

      await waitFor(() => {
        expect(hrefSetter).toHaveBeenCalledWith(
          "http://127.0.0.1:46233/callback?token=cli-jwt-token&state=abc123",
        );
      });
    } finally {
      Object.defineProperty(window, "location", {
        configurable: true,
        value: originalLocation,
      });
    }
  });

  it("rejects an invalid cli_callback without falling back to Web Google login", async () => {
    mockSearchParams.set("state", "cli_callback:https://evil.com/callback");

    render(<CallbackPage />);

    await waitFor(() => {
      expect(screen.getByText("Unsupported login callback")).toBeInTheDocument();
    });
    expect(mockGoogleLogin).not.toHaveBeenCalled();
  });

  it("rejects malformed encoded callback state without throwing or exchanging", async () => {
    mockSearchParams.set("state", "cli_callback:%E0%A4%A");

    render(<CallbackPage />);

    await waitFor(() => {
      expect(screen.getByText("Unsupported login callback")).toBeInTheDocument();
    });
    expect(mockGoogleLogin).not.toHaveBeenCalled();
  });

  it("redirects to CLI callback even when state also contains platform:desktop", async () => {
    // cli_callback takes precedence over platform:desktop — the CLI flow
    // is a specific user intent that should not be derailed by desktop flag.
    const hrefSetter = vi.fn();
    const originalLocation = window.location;
    Object.defineProperty(window, "location", {
      configurable: true,
      writable: true,
      value: { ...originalLocation, set href(value: string) { hrefSetter(value); } },
    });

    try {
      mockSearchParams.set(
        "state",
        "platform:desktop,cli_callback:http://localhost:12345/callback,cli_state:mystate",
      );
      mockGoogleLogin.mockResolvedValue({ token: "mixed-jwt" });

      render(<CallbackPage />);

      await waitFor(() => {
        expect(mockGoogleLogin).toHaveBeenCalled();
      });

      await waitFor(() => {
        expect(hrefSetter).toHaveBeenCalledWith(
          "http://localhost:12345/callback?token=mixed-jwt&state=mystate",
        );
      });
    } finally {
      Object.defineProperty(window, "location", {
        configurable: true,
        value: originalLocation,
      });
    }
  });

  it("does not complete Desktop login on the product web callback", async () => {
    mockSearchParams.set(
      "state",
      "platform:desktop,desktop_state:desktop-state,desktop_code_challenge:desktop-challenge",
    );

    render(<CallbackPage />);

    await waitFor(() => {
      expect(screen.getByText("Unsupported login callback")).toBeInTheDocument();
    });
    expect(mockGoogleLogin).not.toHaveBeenCalled();
    expect(mockCompleteDesktopAuthHandoff).not.toHaveBeenCalled();
    expect(screen.queryByText("Opening Patchbay")).not.toBeInTheDocument();
  });

});
