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
const capturedConfirm = vi.hoisted(() => ({
  current: null as { onOk: () => Promise<unknown> } | null,
}));
// `confirmModal` is imperative and module-level, so the config it is handed —
// and therefore the promise `onOk` returns — is the only place the rethrow
// contract is observable. Recorded here and delegated to the real one, because
// the tests below also drive the dialog through the UI.
const mockConfirmModal = vi.hoisted(() => vi.fn());
const actualConfirmModal = vi.hoisted(() => ({
  current: null as null | ((config: never) => unknown),
}));

vi.mock("@lobehub/ui/base-ui", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@lobehub/ui/base-ui")>();
  actualConfirmModal.current = actual.confirmModal as never;
  return { ...actual, confirmModal: mockConfirmModal };
});

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

/**
 * Whether the confirm dialog has left the document inside the window a
 * dismissal takes. The same instrument as `settings-confirm.test.tsx`'s, and
 * for the same reason: "is it gone" is only meaningful after a wait, and a
 * check that never waits cannot tell open from closing.
 */
async function closedWithinWindow(): Promise<boolean> {
  try {
    await waitFor(() => expect(screen.queryByText("Revoke token")).toBeNull(), {
      timeout: 1_200,
    });
    return true;
  } catch {
    return false;
  }
}

describe("TokensTab", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    tokensRef.current = [];
    capturedConfirm.current = null;
    mockList.mockImplementation(async () => tokensRef.current);
    mockCopyText.mockResolvedValue(true);
    mockConfirmModal.mockImplementation((config: never) => {
      capturedConfirm.current = config as { onOk: () => Promise<unknown> };
      return actualConfirmModal.current?.(config);
    });
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
    // A successful revoke dismisses the dialog, and the dismissal is observable
    // in this file — which is what gives the negative assertion two tests down
    // its meaning. Without this demonstration "the title is still in the
    // document" could be true because nothing ever leaves it.
    expect(await closedWithinWindow()).toBe(true);
  });

  /**
   * The rethrow contract, asserted twice and at two layers, because the
   * DOM-level-only version of this test could not fail.
   *
   * `confirmModal` closes on the line after `onOk` unless `onOk` returns a
   * promise, and stays open when that promise rejects — so a revoke that fails
   * must leave the confirmation up rather than dismiss as though the token were
   * gone.
   *
   * **What was wrong with the original.** It read
   * `expect(screen.getByText("Revoke token")).toBeInTheDocument()` immediately
   * after the error toast. Mutating `tokens-tab.tsx`'s `throw e` to `return`
   * left all five tests green: at that instant the closing dialog is still in
   * the document, so the assertion's subject exists in both the open and the
   * closing state. (Measured, not assumed. Replacing the mutation with a
   * *resolving* request and waiting: the title does leave the document, so the
   * node is not the problem — the missing time window is. `labels-tab.test.tsx`
   * saw the same instant and attributed it to the node outliving the close,
   * which is true of the instant and not of the window.)
   *
   * The first test below keeps the DOM spelling but waits the window in which
   * the success path is shown to close. The second drops the DOM entirely: the
   * contract lives in the promise `onOk` returns, and that is directly readable
   * off the config the wrapper handed the library.
   */
  it("keeps the confirmation up when the revoke request fails", async () => {
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
    expect(await closedWithinWindow()).toBe(false);
  });

  it("hands confirmModal an onOk that rejects when the revoke fails", async () => {
    const user = userEvent.setup();
    tokensRef.current = [TOKEN];
    mockRevoke.mockRejectedValue(new Error("network down"));
    await renderTab();

    // Opening the dialog is enough: `onOk` is captured when the wrapper builds
    // the config, so it can be awaited here without a second revoke call.
    await user.click(await screen.findByRole("button", { name: "Revoke My CLI" }));
    expect(capturedConfirm.current).not.toBeNull();

    await expect(capturedConfirm.current?.onOk()).rejects.toThrow("network down");
    expect(mockToastError).toHaveBeenCalledWith("network down");
  });
});
