// @vitest-environment jsdom

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { renderWithI18n } from "../../test/i18n";

const connectionRef = vi.hoisted(() => ({
  current: {
    configured: true,
    connected: true,
    pull_import_enabled: false,
    push_enabled: true,
    connection: {
      id: "conn-1",
      workspace_id: "ws-1",
      organization_id: "org-1",
      organization_name: "Acme Linear",
      actor_id: "actor-1",
      scopes: [],
      webhook_id: null,
      status: "active",
      token_expires_at: "2099-01-01T00:00:00Z",
      last_success_at: "2026-09-01T00:00:00Z",
      last_error: null,
      created_at: "2026-01-01T00:00:00Z",
      updated_at: "2026-01-01T00:00:00Z",
    },
  },
}));

vi.mock("@orvilo/core/linear", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@orvilo/core/linear")>();
  return {
    ...actual,
    linearConnectionOptions: () => ({
      queryKey: ["linear-connection"],
      queryFn: async () => connectionRef.current,
    }),
    linearBindingsOptions: () => ({
      queryKey: ["linear-bindings"],
      queryFn: async () => ({ bindings: [] }),
    }),
    linearCatalogOptions: (_workspaceId: string, enabled = true) => ({
      queryKey: ["linear-catalog", enabled],
      queryFn: async () => ({ teams: [], projects: [], users: [], labels: [], workflow_states: [] }),
      enabled,
    }),
    linearMemberBindingsOptions: () => ({
      queryKey: ["linear-member-bindings"],
      queryFn: async () => ({ bindings: [] }),
    }),
    linearConflictsOptions: () => ({
      queryKey: ["linear-conflicts"],
      queryFn: async () => ({ conflicts: [] }),
    }),
  };
});

vi.mock("@orvilo/core/projects", () => ({
  projectListOptions: () => ({
    queryKey: ["projects"],
    queryFn: async () => ({ projects: [] }),
  }),
}));

vi.mock("@orvilo/core/workspace/queries", () => ({
  memberListOptions: () => ({
    queryKey: ["members"],
    queryFn: async () => [],
  }),
}));

const mocks = vi.hoisted(() => ({
  connectLinear: vi.fn(),
  disconnectLinear: vi.fn(),
}));

vi.mock("@orvilo/core/api", () => ({
  ApiError: class ApiError extends Error {
    status: number;
    constructor(message: string, status = 500) {
      super(message);
      this.status = status;
    }
  },
  api: {
    connectLinear: mocks.connectLinear,
    disconnectLinear: mocks.disconnectLinear,
  },
}));

/**
 * `confirmModal` is imperative and module-level, so the config the wrapper
 * hands it — and therefore the promise `onOk` returns — is the only place the
 * rethrow contract is observable. Recorded and delegated to the real one,
 * because the tests below also drive the dialog through the UI.
 */
const capturedConfirm = vi.hoisted(() => ({
  current: null as { onOk: () => Promise<unknown> } | null,
}));
const mockConfirmModal = vi.hoisted(() => vi.fn());
const actualConfirmModal = vi.hoisted(() => ({
  current: null as null | ((config: never) => unknown),
}));

vi.mock("@lobehub/ui/base-ui", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@lobehub/ui/base-ui")>();
  actualConfirmModal.current = actual.confirmModal as never;
  return { ...actual, confirmModal: mockConfirmModal };
});

import { LinearIntegrationCard } from "./linear-tab";

/**
 * Whether the confirm dialog leaves the document within `timeout`. Polls,
 * because a check that samples at a fixed moment cannot tell an open dialog
 * from a closing one, and it is addressed by its own copy — the imperative
 * confirm is the base-ui `Modal`, not antd's.
 *
 * The timeout is a parameter because the two callers want opposite things from
 * it: the success path passes a budget (polling returns the instant the node
 * goes, so a generous one is free), the failure path a fixed window (spent in
 * full, and only the one that would catch a mutated build).
 */
async function dialogLeavesWithin(timeout: number): Promise<boolean> {
  try {
    await waitFor(
      () => expect(screen.queryByText("Disconnect Linear?")).toBeNull(),
      { timeout },
    );
    return true;
  } catch {
    return false;
  }
}

/** A dismissal window: long enough for a close, short enough to be an assertion. */
const CLOSE_WINDOW_MS = 1_200;
/** A budget, not a window: the success path must not fail a loaded runner. */
const CLOSE_BUDGET_MS = 8_000;

/** Open the row's overflow menu and press Disconnect, which opens the confirm. */
async function openDisconnectConfirm() {
  fireEvent.click(await screen.findByRole("button", { name: "Manage" }));
  await userEvent.click(await screen.findByRole("menuitem", { name: "Disconnect" }));
  await screen.findByText("Disconnect Linear?");
  const confirmButtons = await screen.findAllByRole("button", { name: /^Disconnect$/ });
  await userEvent.click(confirmButtons.at(-1)!);
}

function renderCard() {
  const qc = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  // `lobe: true` added by Task 8b, and it is the only edit made to this file
  // from there. `LinearIntegrationCard` renders `IntegrationCard`,
  // `IntegrationRowMenu` and `ConnectionDotBadge`, and the row menu's trigger
  // and the confirm dialog are Lobe now — so without the bridge those three
  // suites get a `MotionProvider` throw. Turning the flag on wraps the helper
  // in one more provider; it cannot change what the product code does.
  return renderWithI18n(
    <QueryClientProvider client={qc}>
      <LinearIntegrationCard canManage isGuest={false} workspaceId="ws-1" />
    </QueryClientProvider>,
    { lobe: true },
  );
}

afterEach(() => {
  cleanup();
  connectionRef.current.connection.status = "active";
  connectionRef.current.configured = true;
});

beforeEach(() => {
  vi.clearAllMocks();
  capturedConfirm.current = null;
  mockConfirmModal.mockImplementation((config: never) => {
    capturedConfirm.current = config as { onOk: () => Promise<unknown> };
    return actualConfirmModal.current?.(config);
  });
});

describe("LinearIntegrationCard", () => {
  it("uses the IM connection badge and kebab instead of Authorized / inline buttons", async () => {
    renderCard();

    await waitFor(() => {
      expect(screen.getByText("Connected")).toBeInTheDocument();
    });
    expect(screen.queryByText("Authorized")).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Disconnect" })).not.toBeInTheDocument();
    expect(screen.queryByText("Acme Linear")).not.toBeInTheDocument();
    expect(screen.queryByText("Loading Linear catalog")).not.toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "Manage" }));
    expect(screen.getByRole("menuitem", { name: "Manage" })).toBeInTheDocument();
    expect(screen.getByRole("menuitem", { name: "Disconnect" })).toBeInTheDocument();
  });

  it("does not mount the catalog dialog until manage is chosen", async () => {
    renderCard();
    await waitFor(() => {
      expect(screen.getByText("Connected")).toBeInTheDocument();
    });
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
  });

  /**
   * The caller's half of the `confirmModal` contract.
   *
   * `confirmModal` closes on the line after `onOk` unless `onOk` returns a
   * promise, and stays open when that promise rejects — so a disconnect that
   * fails must leave the confirmation up rather than dismiss as though the
   * connection were gone. The first case shows the success path *does* close,
   * which is what gives the second one's negative its meaning; the third reads
   * the contract off the config the wrapper handed the library, where it is not
   * a DOM question at all.
   */
  it("disconnects only after the confirmation, and the dialog closes when it succeeds", async () => {
    mocks.disconnectLinear.mockResolvedValue(undefined);
    renderCard();
    await screen.findByRole("button", { name: "Manage" });
    await openDisconnectConfirm();
    await waitFor(() => expect(mocks.disconnectLinear).toHaveBeenCalledWith("ws-1"));
    expect(await dialogLeavesWithin(CLOSE_BUDGET_MS)).toBe(true);
  });

  it("keeps the confirmation up when the disconnect request fails", async () => {
    mocks.disconnectLinear.mockRejectedValue(new Error("network failed"));
    renderCard();
    await screen.findByRole("button", { name: "Manage" });
    await openDisconnectConfirm();

    // The request really was attempted — otherwise "the dialog is still up"
    // could be satisfied by a click that never reached the handler.
    await waitFor(() => expect(mocks.disconnectLinear).toHaveBeenCalledWith("ws-1"));
    expect(await dialogLeavesWithin(CLOSE_WINDOW_MS)).toBe(false);

    // Deliberately still open, and the confirm stack is module state that
    // outlives this case — an open dialog would land on the next one in this
    // file and read exactly like a flake. Close it and wait for it to go.
    await userEvent.click(screen.getByRole("button", { name: "Cancel" }));
    expect(await dialogLeavesWithin(CLOSE_BUDGET_MS)).toBe(true);
  });

  it("hands confirmModal an onOk that rejects when the disconnect fails", async () => {
    mocks.disconnectLinear.mockRejectedValue(new Error("network failed"));
    renderCard();
    await screen.findByRole("button", { name: "Manage" });
    fireEvent.click(screen.getByRole("button", { name: "Manage" }));
    await userEvent.click(await screen.findByRole("menuitem", { name: "Disconnect" }));
    expect(capturedConfirm.current).not.toBeNull();

    await expect(capturedConfirm.current?.onOk()).rejects.toThrow("network failed");
  });
});
