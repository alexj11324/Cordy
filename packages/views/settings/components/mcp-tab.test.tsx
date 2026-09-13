// @vitest-environment jsdom

import { beforeEach, describe, expect, it, vi } from "vitest";
import { screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { renderWithI18n } from "../../test/i18n";
import enSettings from "../../locales/en/settings.json";

const mockCreate = vi.hoisted(() => vi.fn());
const mockUpdate = vi.hoisted(() => vi.fn());
const mockDelete = vi.hoisted(() => vi.fn());
const mockToastError = vi.hoisted(() => vi.fn());

// The wrapper hands `confirmModal` a config this suite can read, and delegates
// to the real one so the dialog is still driven through the UI. That is what
// makes the promise contract assertable at all: whether `onOk` rejects is not a
// DOM question.
const mockConfirmModal = vi.hoisted(() => vi.fn());
const actualConfirmModal = vi.hoisted(
  () => ({ current: null as null | ((config: never) => unknown) }),
);
const capturedConfirm = vi.hoisted(
  () => ({ current: null as null | { onOk: () => Promise<unknown> } }),
);

vi.mock("@lobehub/ui/base-ui", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@lobehub/ui/base-ui")>();
  actualConfirmModal.current = actual.confirmModal as never;
  return { ...actual, confirmModal: mockConfirmModal };
});

const server = (over: Record<string, unknown>) => ({
  id: "srv-1",
  workspace_id: "workspace-1",
  name: "linear",
  transport: "http",
  created_at: "2026-08-14T00:00:00Z",
  updated_at: "2026-08-14T00:00:00Z",
  ...over,
});

const data = vi.hoisted(() => ({
  servers: [] as Array<Record<string, unknown>>,
  isLoading: false,
  role: "owner" as "owner" | "admin" | "member",
}));

vi.mock("@tanstack/react-query", () => ({
  useQuery: () => ({ data: data.servers, isLoading: data.isLoading }),
}));

vi.mock("@orvilo/core/workspace/queries", () => ({
  workspaceMcpServersOptions: () => ({ queryKey: ["workspaces", "workspace-1", "mcp-servers"] }),
}));

vi.mock("@orvilo/core/workspace/mutations", () => ({
  useCreateWorkspaceMcpServer: () => ({ mutateAsync: mockCreate, isPending: false }),
  useUpdateWorkspaceMcpServer: () => ({ mutateAsync: mockUpdate, isPending: false }),
  useDeleteWorkspaceMcpServer: () => ({ mutateAsync: mockDelete, isPending: false }),
}));

vi.mock("@orvilo/core/paths", () => ({
  useCurrentWorkspace: () => ({ id: "workspace-1", name: "Acme", slug: "acme" }),
}));

vi.mock("@orvilo/core/permissions", () => ({
  useCurrentMember: () => ({ role: data.role, isLoading: false }),
}));

vi.mock("sonner", () => ({ toast: { success: vi.fn(), error: mockToastError } }));

import { McpTab } from "./mcp-tab";

// `{ lobe: true }` is required, not decoration: this suite used a bare `render`
// with its own `I18nProvider` and never reached `LobeThemeBridge`, while the
// tab is Lobe now — and the imperative confirm needs the bridge's `ModalHost`
// mounted, or `confirmModal` pushes onto a stack nothing renders.
//
// Async because the bridge is lazy: until its module resolves the tree is a
// `Suspense` fallback of `null`. The tab's lede renders on every path.
async function renderTab() {
  const result = renderWithI18n(<McpTab />, { lobe: true });
  await screen.findByText(enSettings.mcp.description);
  return result;
}

/**
 * Whether the confirmation leaves the document within `timeout`. Polls,
 * because a check that samples at a fixed moment cannot tell an open dialog
 * from a closing one.
 *
 * The timeout is a parameter because the two callers want opposite things from
 * it: the success path passes a budget (polling returns the instant the node
 * goes, so a generous one is free), the failure path a fixed window (spent in
 * full, and only the one that would catch a mutated build).
 */
async function dialogLeavesWithin(timeout: number): Promise<boolean> {
  try {
    await waitFor(
      () => expect(screen.queryByText(enSettings.mcp.delete_title)).toBeNull(),
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

describe("McpTab", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    capturedConfirm.current = null;
    mockConfirmModal.mockImplementation((config: never) => {
      capturedConfirm.current = config as { onOk: () => Promise<unknown> };
      return actualConfirmModal.current?.(config);
    });
    data.role = "owner";
    data.isLoading = false;
    data.servers = [
      server({ id: "srv-1", name: "linear", transport: "http" }),
      server({ id: "srv-2", name: "local-tool", transport: "stdio" }),
    ];
    mockCreate.mockResolvedValue({});
    mockUpdate.mockResolvedValue({});
    mockDelete.mockResolvedValue({});
  });

  it("lists the library servers with their transport", async () => {
    await renderTab();

    expect(screen.getByText("linear")).toBeInTheDocument();
    expect(screen.getByText("HTTP")).toBeInTheDocument();
    expect(screen.getByText("local-tool")).toBeInTheDocument();
    expect(screen.getByText("stdio")).toBeInTheDocument();
  });

  it("adds a server to the library", async () => {
    const user = userEvent.setup();
    await renderTab();

    await user.click(screen.getByRole("button", { name: /Add server/ }));
    await user.type(screen.getByLabelText("Name"), "github");
    // The shared dialog defaults to the STDIO transport.
    await user.type(screen.getByLabelText("Command"), "github-mcp");
    await user.click(screen.getByRole("button", { name: "Add Server" }));

    await waitFor(() =>
      expect(mockCreate).toHaveBeenCalledWith(
        expect.objectContaining({ name: "github" }),
      ),
    );
    // The entry is sent on its own — the client never holds, and therefore
    // never resends, anything else.
    const call = mockCreate.mock.calls[0]![0] as { config: Record<string, unknown> };
    expect(call.config).not.toHaveProperty("mcpServers");
  });

  // Renaming is safe in this model — assignments key off the server id — so
  // the name field stays editable and the update targets the opened entry.
  it("edits a library server by id, so a rename keeps its assignments", async () => {
    const user = userEvent.setup();
    await renderTab();

    await user.click(screen.getAllByRole("button", { name: "Edit server" })[0]!);

    const nameInput = screen.getByLabelText("Name") as HTMLInputElement;
    expect(nameInput.value).toBe("linear");
    expect(nameInput).not.toHaveAttribute("readonly");

    await user.clear(nameInput);
    await user.type(nameInput, "linear-v2");
    await user.type(screen.getByLabelText("Server URL"), "https://linear-v2.example");
    await user.click(screen.getByRole("button", { name: "Save Changes" }));

    await waitFor(() =>
      expect(mockUpdate).toHaveBeenCalledWith(
        expect.objectContaining({ serverId: "srv-1", name: "linear-v2" }),
      ),
    );
    expect(mockCreate).not.toHaveBeenCalled();
  });

  // The saved entry cannot be read back, but the safe summary still knows the
  // transport — so the empty form must open on the right one instead of
  // defaulting a stdio server to HTTP.
  it("opens the edit form on the server's own transport", async () => {
    const user = userEvent.setup();
    await renderTab();

    // Row 1 is the stdio server.
    await user.click(screen.getAllByRole("button", { name: "Edit server" })[1]!);

    expect(screen.getByRole("button", { name: "STDIO" })).toHaveAttribute(
      "aria-pressed",
      "true",
    );
    expect(screen.getByLabelText("Command")).toBeInTheDocument();
  });

  // Regression: the guided form only speaks stdio/http and REWRITES the entry
  // on save (`type: "http"`), so editing an SSE server through it would change
  // its protocol and can break the server. The server returns `sse`, so this is
  // reachable, not hypothetical — those entries take the JSON path instead.
  it.each(["sse", "websocket"])(
    "edits a %s server through JSON, never the transport-rewriting form",
    async (transport) => {
      const user = userEvent.setup();
      data.servers = [server({ name: "streamy", transport })];
      await renderTab();

      await user.click(screen.getByRole("button", { name: "Edit server" }));

      expect(screen.getByRole("tab", { name: "JSON" })).toHaveAttribute(
        "aria-selected",
        "true",
      );
      // Not merely unselected: switching to the form would rewrite the entry.
      expect(screen.getByRole("tab", { name: "Form" })).toHaveAttribute(
        "aria-disabled",
        "true",
      );
      expect(screen.queryByLabelText("Server URL")).toBeNull();
    },
  );

  /**
   * The caller's half of the `confirmModal` contract.
   *
   * `confirmModal` closes on the line after `onOk` unless `onOk` returns a
   * promise, and stays open when that promise rejects — so a removal that fails
   * must leave the confirmation up rather than dismiss as though the server
   * were gone. The first case shows the success path *does* close, which is
   * what gives the second one's negative its meaning; the third reads the
   * contract off the config the wrapper handed the library, where it is not a
   * DOM question at all.
   */
  it("removes a server only after the confirmation, and the dialog closes when it succeeds", async () => {
    const user = userEvent.setup();
    await renderTab();

    await user.click(screen.getAllByRole("button", { name: "Remove server" })[0]!);
    expect(mockDelete).not.toHaveBeenCalled();

    await screen.findByText(enSettings.mcp.delete_title);
    await user.click(screen.getByRole("button", { name: "Remove" }));

    await waitFor(() => expect(mockDelete).toHaveBeenCalledWith("srv-1"));
    expect(await dialogLeavesWithin(CLOSE_BUDGET_MS)).toBe(true);
  });

  it("keeps the confirmation up when the removal request fails", async () => {
    mockDelete.mockRejectedValue(new Error("network failed"));
    const user = userEvent.setup();
    await renderTab();

    await user.click(screen.getAllByRole("button", { name: "Remove server" })[0]!);
    await screen.findByText(enSettings.mcp.delete_title);
    await user.click(screen.getByRole("button", { name: "Remove" }));

    await waitFor(() => expect(mockToastError).toHaveBeenCalledWith("network failed"));
    expect(await dialogLeavesWithin(CLOSE_WINDOW_MS)).toBe(false);

    // It is deliberately still open, and the confirm stack is module state that
    // outlives this case — an open dialog would land on the next one in this
    // file and read exactly like a flake. Close it and wait for it to go.
    await user.click(screen.getByRole("button", { name: "Cancel" }));
    expect(await dialogLeavesWithin(CLOSE_BUDGET_MS)).toBe(true);
  });

  it("hands confirmModal an onOk that rejects when the removal fails", async () => {
    mockDelete.mockRejectedValue(new Error("network failed"));
    const user = userEvent.setup();
    await renderTab();

    await user.click(screen.getAllByRole("button", { name: "Remove server" })[0]!);
    expect(capturedConfirm.current).not.toBeNull();

    await expect(capturedConfirm.current!.onOk()).rejects.toThrow("network failed");
    expect(mockToastError).toHaveBeenCalledWith("network failed");

    // Same reason as the case above: this one read the config directly, so the
    // dialog it opened is still on the stack and has to be taken off it.
    await user.click(screen.getByRole("button", { name: "Cancel" }));
    expect(await dialogLeavesWithin(CLOSE_BUDGET_MS)).toBe(true);
  });

  it("hides every write affordance from a plain member", async () => {
    data.role = "member";
    await renderTab();

    // The inventory itself stays visible — it carries no credential material.
    expect(screen.getByText("linear")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /Add server/ })).toBeNull();
    expect(screen.queryByRole("button", { name: "Edit server" })).toBeNull();
    expect(screen.queryByRole("button", { name: "Remove server" })).toBeNull();
    expect(
      screen.getByText(/Only workspace owners and admins/),
    ).toBeInTheDocument();
  });

  it("renders an empty state when the library is empty", async () => {
    data.servers = [];
    await renderTab();

    expect(screen.getByText("No shared MCP servers")).toBeInTheDocument();
  });

  // The document is write-only, so the screen must never imply it is showing
  // a saved configuration: it says an edit replaces the entry.
  it("states that saved configurations are write-only", async () => {
    await renderTab();

    expect(screen.getByText(/write-only/)).toBeInTheDocument();
  });

  it("survives a payload that is not an array", async () => {
    // Backend drift: the schema defaults the list to [], but the component
    // must not crash if it ever arrives undefined.
    data.servers = undefined as unknown as typeof data.servers;
    await renderTab();

    expect(screen.getByText("No shared MCP servers")).toBeInTheDocument();
  });
});
