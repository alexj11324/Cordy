import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

const mockList = vi.hoisted(() => vi.fn());
const mockCreate = vi.hoisted(() => vi.fn());
const mockRevoke = vi.hoisted(() => vi.fn());
const mockCopyText = vi.hoisted(() => vi.fn());
const mockToastSuccess = vi.hoisted(() => vi.fn());
const mockToastError = vi.hoisted(() => vi.fn());
const tokensRef = vi.hoisted(() => ({ current: [] as unknown[] }));

vi.mock("@orvilo/core/api", () => ({
  api: {
    listPersonalAccessTokens: mockList,
    createPersonalAccessToken: mockCreate,
    revokePersonalAccessToken: mockRevoke,
  },
}));

vi.mock("@orvilo/ui/lib/clipboard", () => ({ copyText: mockCopyText }));

vi.mock("sonner", () => ({
  toast: { success: mockToastSuccess, error: mockToastError },
}));

import { renderWithI18n } from "../../test/i18n";
import { TokensTab } from "./tokens-tab";

vi.setConfig({ testTimeout: 60_000, hookTimeout: 30_000 });

const TOKEN = {
  id: "token-1",
  name: "My CLI",
  token_prefix: "orv_abc",
  created_at: "2026-01-02T03:04:05.000Z",
  last_used_at: null,
  expires_at: null,
};

/** `lobe: true` loads the theme bridge on demand, so the first query is async. */
async function renderTab() {
  renderWithI18n(<TokensTab />, { lobe: true });
  await screen.findByPlaceholderText("Token name (e.g. My CLI)");
}

describe("TokensTab", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    tokensRef.current = [];
    mockList.mockImplementation(async () => tokensRef.current);
    mockCopyText.mockResolvedValue(true);
  });

  afterEach(() => {
    cleanup();
  });

  it("shows the empty state when no token is issued", async () => {
    await renderTab();
    expect(
      await screen.findByText("No authorized clients yet."),
    ).toBeInTheDocument();
  });

  it("lists the issued tokens with their metadata", async () => {
    tokensRef.current = [TOKEN];
    await renderTab();

    expect(await screen.findByText("My CLI")).toBeInTheDocument();
    expect(screen.getByText(/orv_abc\.\.\./)).toBeInTheDocument();
  });

  it("creates a token and shows it once in the created dialog", async () => {
    const user = userEvent.setup();
    mockCreate.mockResolvedValue({ token: "orv_full_secret" });
    await renderTab();

    await user.type(
      screen.getByPlaceholderText("Token name (e.g. My CLI)"),
      "My CLI",
    );
    await user.click(screen.getByRole("button", { name: "Create" }));

    await waitFor(() => {
      expect(mockCreate).toHaveBeenCalledWith({
        name: "My CLI",
        expires_in_days: 90,
      });
    });
    expect(
      await screen.findByText("Personal access token created"),
    ).toBeInTheDocument();
    expect(screen.getByText("orv_full_secret")).toBeInTheDocument();
  });

  it("revokes through the confirm modal only after it is confirmed", async () => {
    const user = userEvent.setup();
    tokensRef.current = [TOKEN];
    mockRevoke.mockResolvedValue(undefined);
    await renderTab();

    await user.click(await screen.findByRole("button", { name: "Revoke My CLI" }));
    expect(mockRevoke).not.toHaveBeenCalled();

    await screen.findByText("Revoke token");
    await user.click(screen.getByRole("button", { name: "Revoke" }));

    await waitFor(() => {
      expect(mockRevoke).toHaveBeenCalledWith("token-1");
    });
    expect(mockToastSuccess).toHaveBeenCalledWith("Token revoked");
  });

  /**
   * The rethrow contract. `confirmModal` closes on the line after `onOk`
   * unless `onOk` returns a promise, and stays open when that promise rejects —
   * so a revoke that fails must leave the confirmation up rather than dismiss
   * as though the token were gone.
   */
  it("keeps the confirmation open when the revoke request fails", async () => {
    const user = userEvent.setup();
    tokensRef.current = [TOKEN];
    mockRevoke.mockRejectedValue(new Error("network down"));
    await renderTab();

    await user.click(await screen.findByRole("button", { name: "Revoke My CLI" }));
    await screen.findByText("Revoke token");
    await user.click(screen.getByRole("button", { name: "Revoke" }));

    await waitFor(() => {
      expect(mockToastError).toHaveBeenCalledWith("network down");
    });
    expect(screen.getByText("Revoke token")).toBeInTheDocument();
  });
});
