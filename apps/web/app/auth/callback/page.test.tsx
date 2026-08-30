import { render, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

const { mockReplace, search } = vi.hoisted(() => ({
  mockReplace: vi.fn(),
  search: { current: "" },
}));

vi.mock("next/navigation", () => ({
  useRouter: () => ({ replace: mockReplace }),
  useSearchParams: () => new URLSearchParams(search.current),
}));

import AuthCallbackPage from "./page";

describe("AuthCallbackPage", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    search.current = "";
    delete process.env.NEXT_PUBLIC_DESKTOP_APP_ORIGIN;
  });

  it("returns a Clerk-established session to the app root", async () => {
    render(<AuthCallbackPage />);

    await waitFor(() => expect(mockReplace).toHaveBeenCalledWith("/"));
  });

  it("returns a desktop handoff to the login page", async () => {
    search.current = "platform=desktop";

    render(<AuthCallbackPage />);

    await waitFor(() =>
      expect(mockReplace).toHaveBeenCalledWith("/login?platform=desktop"),
    );
  });

  it("preserves the allowlisted browser origin with desktop PKCE state", async () => {
    process.env.NEXT_PUBLIC_DESKTOP_APP_ORIGIN = "https://app.patchbay.ai";
    search.current =
      "platform=desktop&code_challenge=challenge-value&state=opaque-state" +
      "&app_origin=https%3A%2F%2Fapp.patchbay.ai";

    render(<AuthCallbackPage />);

    await waitFor(() =>
      expect(mockReplace).toHaveBeenCalledWith(
        "/login?platform=desktop&code_challenge=challenge-value" +
          "&state=opaque-state&app_origin=https%3A%2F%2Fapp.patchbay.ai",
      ),
    );
  });
});
