import { describe, it, expect, beforeEach, vi } from "vitest";
import { StrictMode } from "react";
import { screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import enSettings from "../../locales/en/settings.json";
import type { ComposioConnection, ComposioToolkit } from "@orvilo/core/types";

// --- Mutable refs the mocked hooks read from, so each test can shape the data
// without re-mocking the modules. ---
const toolkitsRef = vi.hoisted(() => ({
  current: { data: [] as ComposioToolkit[], isLoading: false, isError: false },
}));
const connectionsRef = vi.hoisted(() => ({
  current: { data: [] as ComposioConnection[], isError: false },
}));
const searchParamsRef = vi.hoisted(() => ({ current: new URLSearchParams("tab=integrations") }));

const mockInvalidate = vi.hoisted(() => vi.fn());
const mockReplace = vi.hoisted(() => vi.fn());
const mockToastSuccess = vi.hoisted(() => vi.fn());
const mockToastError = vi.hoisted(() => vi.fn());
const mockDeleteConnection = vi.hoisted(() => vi.fn());

/**
 * `confirmModal` is imperative and module-level, so the config the wrapper
 * hands it — and therefore the promise `onOk` returns — is the only place the
 * rethrow contract is observable. Recorded here and delegated to the real one,
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

vi.mock("@tanstack/react-query", () => ({
  useQuery: (opts: { queryKey: unknown[] }) => {
    const key = JSON.stringify(opts.queryKey);
    if (key.includes("toolkits")) return toolkitsRef.current;
    if (key.includes("connections")) return connectionsRef.current;
    return { data: undefined };
  },
  useQueryClient: () => ({ invalidateQueries: mockInvalidate }),
  queryOptions: <T,>(opts: T) => opts,
}));

vi.mock("@orvilo/core/composio", () => ({
  composioKeys: {
    all: ["composio"],
    toolkits: () => ["composio", "toolkits"],
    connections: () => ["composio", "connections"],
  },
  composioToolkitsOptions: () => ({ queryKey: ["composio", "toolkits"], queryFn: vi.fn() }),
  composioConnectionsOptions: () => ({ queryKey: ["composio", "connections"], queryFn: vi.fn() }),
}));

vi.mock("@orvilo/core/api", () => ({
  api: {
    beginComposioConnect: vi.fn(),
    deleteComposioConnection: mockDeleteConnection,
  },
}));

vi.mock("../../navigation", () => ({
  useNavigation: () => ({
    push: vi.fn(),
    replace: mockReplace,
    back: vi.fn(),
    pathname: "/acme/settings",
    searchParams: searchParamsRef.current,
    hash: "",
    getShareableUrl: (p: string) => `https://app.example${p}`,
  }),
}));

vi.mock("sonner", () => ({
  toast: { success: mockToastSuccess, error: mockToastError },
}));

import { renderWithI18n } from "../../test/i18n";
import { ComposioTab } from "./composio-tab";

/**
 * `lobe: true` mounts the theme bridge on demand, so everything after a
 * bridged render must start with an async query — until the bridge's module
 * resolves the tree is a `Suspense` fallback of `null` and a synchronous
 * `getBy*` would fail. Everything this tab renders is Lobe or sits inside the
 * bridge (`settings-page.tsx` mounts it around the whole dialog body), so the
 * whole file opts in.
 */
function renderTab() {
  return renderWithI18n(<ComposioTab />, { lobe: true });
}

// StrictMode reproduces React's dev-mode mount → cleanup → mount double-invoke,
// which is exactly what would double-fire the callback toast without the
// consumed-key ref guard.
function renderTabStrict() {
  return renderWithI18n(
    <StrictMode>
      <ComposioTab />
    </StrictMode>,
    { lobe: true },
  );
}

/**
 * Whether the confirm dialog leaves the document within `timeout`. Polls —
 * `waitFor` re-runs the callback until it stops throwing — because a check that
 * samples at a fixed moment cannot tell an open dialog from a closing one.
 *
 * The timeout is a parameter because the two callers want opposite things from
 * it: the **success** path passes a generous budget (polling stops the moment
 * the node goes, so a slow close cannot fail it), and the **failure** path
 * passes a fixed window it spends in full (no correct run ever closes, so the
 * only question is whether a mutated build that closes would be caught).
 */
async function dialogLeavesWithin(timeout: number): Promise<boolean> {
  try {
    await waitFor(
      () => expect(screen.queryByText("Disconnect this app?")).toBeNull(),
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

const NOTION: ComposioToolkit = {
  slug: "notion",
  name: "Notion",
  connectable: true,
};

const ACTIVE_CONNECTION: ComposioConnection = {
  id: "conn-1",
  toolkit_slug: "notion",
  status: "active",
  connected_at: "2026-06-01T00:00:00Z",
  last_used_at: null,
};

beforeEach(() => {
  vi.clearAllMocks();
  capturedConfirm.current = null;
  mockConfirmModal.mockImplementation((config: never) => {
    capturedConfirm.current = config as { onOk: () => Promise<unknown> };
    return actualConfirmModal.current?.(config);
  });
  toolkitsRef.current = { data: [NOTION], isLoading: false, isError: false };
  connectionsRef.current = { data: [], isError: false };
  searchParamsRef.current = new URLSearchParams("tab=integrations");
});

describe("ComposioTab", () => {
  it("renders a connected card with a 'never used' placeholder when last_used_at is null", async () => {
    connectionsRef.current = { data: [ACTIVE_CONNECTION], isError: false };
    renderTab();
    expect(await screen.findByText(enSettings.composio.connected)).toBeInTheDocument();
    expect(screen.getByText(enSettings.composio.last_used_never)).toBeInTheDocument();
  });

  it("renders a 'Last used' line when last_used_at is present", async () => {
    connectionsRef.current = {
      data: [
        {
          ...ACTIVE_CONNECTION,
          last_used_at: new Date(Date.now() - 2 * 60 * 1000).toISOString(),
        },
      ],
      isError: false,
    };
    renderTab();
    // "Last used {{when}}" → relative time formatter yields "2m ago"
    expect(await screen.findByText(/Last used/)).toBeInTheDocument();
    expect(screen.queryByText(enSettings.composio.last_used_never)).not.toBeInTheDocument();
  });

  it("renders the expired branch with a Reconnect button", async () => {
    connectionsRef.current = {
      data: [{ ...ACTIVE_CONNECTION, status: "expired" }],
      isError: false,
    };
    renderTab();
    expect(await screen.findByText(enSettings.composio.expired)).toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: new RegExp(enSettings.composio.reconnect) }),
    ).toBeInTheDocument();
    // Not treated as connected, so no Connected badge.
    expect(screen.queryByText(enSettings.composio.connected)).not.toBeInTheDocument();
  });

  it("toasts success and clears the ?connected param on a successful callback", async () => {
    searchParamsRef.current = new URLSearchParams("tab=integrations&connected=notion");
    renderTab();
    await waitFor(() => {
      expect(mockToastSuccess).toHaveBeenCalledWith(enSettings.composio.toast_connected);
    });
    expect(mockInvalidate).toHaveBeenCalledWith({ queryKey: ["composio", "connections"] });
    // The one-shot param is stripped while ?tab is preserved.
    expect(mockReplace).toHaveBeenCalledWith("/acme/settings?tab=integrations");
  });

  it("toasts error on a failed callback", async () => {
    searchParamsRef.current = new URLSearchParams("tab=integrations&error=composio_connect_failed");
    renderTab();
    await waitFor(() => {
      expect(mockToastError).toHaveBeenCalledWith(enSettings.composio.toast_connect_failed);
    });
    expect(mockReplace).toHaveBeenCalledWith("/acme/settings?tab=integrations");
  });

  it("fires the success callback exactly once under StrictMode double-invoke", async () => {
    searchParamsRef.current = new URLSearchParams("tab=integrations&connected=notion");
    renderTabStrict();
    await waitFor(() => {
      expect(mockToastSuccess).toHaveBeenCalled();
    });
    // The consumed-key ref must suppress the second (cleanup → re-mount) run.
    expect(mockToastSuccess).toHaveBeenCalledTimes(1);
    expect(mockInvalidate).toHaveBeenCalledTimes(1);
  });

  /**
   * The rethrow contract, asserted twice and at two layers, because the
   * DOM-level-only version of this test could not fail.
   *
   * `confirmModal` closes on the line after `onOk` unless `onOk` returns a
   * promise, and stays open when that promise rejects — so a disconnect that
   * fails must leave the confirmation up rather than dismiss as though the
   * connection were gone. The first test below keeps the DOM spelling but waits
   * a window in which the success path is shown to close; without that
   * demonstration "the title is still in the document" could be true because
   * nothing ever leaves it. The second drops the DOM entirely: the contract
   * lives in the promise `onOk` returns, and that is directly readable off the
   * config the wrapper handed the library.
   */
  it("disconnects only after the confirmation, and the dialog closes when it succeeds", async () => {
    mockDeleteConnection.mockResolvedValue(undefined);
    connectionsRef.current = { data: [ACTIVE_CONNECTION], isError: false };
    renderTab();
    await userEvent.click(
      await screen.findByRole("button", { name: enSettings.composio.disconnect }),
    );
    expect(mockDeleteConnection).not.toHaveBeenCalled();

    await screen.findByText("Disconnect this app?");
    // Two buttons carry this name — the tile's icon button and the dialog's OK
    // — and the dialog's is last in document order (it is portalled to the end
    // of `<body>`), so `.at(-1)` is the one that runs the request.
    const confirmButtons = await screen.findAllByRole("button", {
      name: new RegExp(`^${enSettings.composio.disconnect}$`),
    });
    await userEvent.click(confirmButtons.at(-1)!);
    await waitFor(() => {
      expect(mockDeleteConnection).toHaveBeenCalledWith("conn-1");
    });
    expect(mockInvalidate).toHaveBeenCalledWith({ queryKey: ["composio", "connections"] });
    expect(await dialogLeavesWithin(CLOSE_BUDGET_MS)).toBe(true);
  });

  it("keeps the confirmation up when the disconnect request fails", async () => {
    mockDeleteConnection.mockRejectedValue(new Error("network failed"));
    connectionsRef.current = { data: [ACTIVE_CONNECTION], isError: false };
    renderTab();
    await userEvent.click(
      await screen.findByRole("button", { name: enSettings.composio.disconnect }),
    );
    await screen.findByText("Disconnect this app?");
    const confirmButtons = await screen.findAllByRole("button", {
      name: new RegExp(`^${enSettings.composio.disconnect}$`),
    });
    await userEvent.click(confirmButtons.at(-1)!);

    await waitFor(() => expect(mockToastError).toHaveBeenCalledWith("network failed"));
    expect(await dialogLeavesWithin(CLOSE_WINDOW_MS)).toBe(false);
  });

  it("hands confirmModal an onOk that rejects when the disconnect fails", async () => {
    mockDeleteConnection.mockRejectedValue(new Error("network failed"));
    connectionsRef.current = { data: [ACTIVE_CONNECTION], isError: false };
    renderTab();
    await userEvent.click(
      await screen.findByRole("button", { name: enSettings.composio.disconnect }),
    );
    expect(capturedConfirm.current).not.toBeNull();

    await expect(capturedConfirm.current?.onOk()).rejects.toThrow("network failed");
    expect(mockToastError).toHaveBeenCalledWith("network failed");
  });
});
