import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

const mockCreate = vi.hoisted(() => vi.fn());
const mockUpdate = vi.hoisted(() => vi.fn());
const mockDelete = vi.hoisted(() => vi.fn());
const mockToastError = vi.hoisted(() => vi.fn());
const labelsRef = vi.hoisted(() => ({ current: [] as unknown[] }));

vi.mock("@orvilo/core/hooks", () => ({
  useWorkspaceId: () => "workspace-1",
}));

vi.mock("@orvilo/core/labels", () => ({
  labelListOptions: (wsId: string, resourceType: string) => ({
    queryKey: ["labels", wsId, "list", resourceType],
  }),
  useCreateLabel: () => ({ mutate: mockCreate, isPending: false }),
  useUpdateLabel: () => ({ mutate: mockUpdate, isPending: false }),
  useDeleteLabel: () => ({ mutate: mockDelete, mutateAsync: mockDelete, isPending: false }),
}));

vi.mock("@tanstack/react-query", () => ({
  useQuery: () => ({ data: labelsRef.current, isLoading: false }),
}));

vi.mock("sonner", () => ({
  toast: { error: mockToastError, success: vi.fn() },
}));

// `confirmModal` is imperative and module-level; capturing the config it was
// handed is the only place its `onOk` contract is observable. See the test.
const mockConfirmModal = vi.hoisted(() => vi.fn());
vi.mock("@lobehub/ui/base-ui", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@lobehub/ui/base-ui")>();
  return { ...actual, confirmModal: mockConfirmModal };
});

import { renderWithI18n } from "../../test/i18n";
import { LabelsTab } from "./labels-tab";

vi.setConfig({ testTimeout: 60_000, hookTimeout: 30_000 });

const LABEL = {
  id: "label-1",
  name: "Bug",
  description: "Something is broken",
  color: "#ef4444",
  resource_type: "issue",
  usage_count: 3,
  updated_at: "2026-01-02T03:04:05.000Z",
};

/**
 * `lobe: true` loads the theme bridge on demand, so the first query is async —
 * and it is scoped to the tab's group, which is the only handle that tells two
 * same-named rows on this page apart. See the test notes in
 * `reference-lobe-tab-migration.md` for why the group query is worth it.
 */
async function renderTab() {
  renderWithI18n(<LabelsTab />, { lobe: true });
  return within(await screen.findByRole("group", { name: "Labels" }));
}

describe("LabelsTab", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    labelsRef.current = [];
  });

  afterEach(cleanup);

  // Agent labels were removed from the product (MUL-5600). The backend still
  // models the `agent` resource type, so the guard here is that the settings
  // UI never offers it as a manageable catalog again.
  it("offers only the issue and skill catalogs", async () => {
    const group = await renderTab();

    expect(group.getByRole("button", { name: /Issues/ })).toBeInTheDocument();
    expect(group.getByRole("button", { name: /Skills/ })).toBeInTheDocument();
    expect(group.queryByRole("button", { name: /Agents/ })).toBeNull();
  });

  it("describes the tab without promising an agent catalog", async () => {
    const group = await renderTab();

    expect(
      group.getByText(/organize issues and skills/i),
    ).toBeInTheDocument();
  });

  /**
   * The `action` regression. This tab's create control used to be handed to
   * `SettingsTab`, whose nested branch returns before rendering `action` — so
   * inside the settings dialog, the only surface this tab has, it did not
   * exist. It lives on the group's `extra` now, and this asserts what the
   * running renderer was measured to show: the control renders.
   */
  it("renders the create-label control", async () => {
    const group = await renderTab();

    expect(
      group.getByRole("button", { name: /New label/ }),
    ).toBeInTheDocument();
  });

  it("lists a label with its description", async () => {
    labelsRef.current = [LABEL];
    const group = await renderTab();

    expect(group.getByText("Bug")).toBeInTheDocument();
    expect(group.getByText("Something is broken")).toBeInTheDocument();
  });

  it("opens a row's overflow menu with edit and delete", async () => {
    const user = userEvent.setup();
    labelsRef.current = [LABEL];
    const group = await renderTab();

    await user.click(group.getByRole("button", { name: "Actions for Bug" }));

    expect(await screen.findByRole("menuitem", { name: /Edit/ })).toBeInTheDocument();
    expect(screen.getByRole("menuitem", { name: /Delete/ })).toBeInTheDocument();
  });

  /**
   * What this tab owes the confirmation: the copy, the destructive tone, and a
   * request that runs only when the user confirms. The dialog's own rendering
   * and its localized buttons belong to `settings-confirm` and are asserted in
   * its suite — this is the layer that can still get them wrong.
   */
  it("deletes only after the confirmation is accepted", async () => {
    const user = userEvent.setup();
    labelsRef.current = [LABEL];
    mockDelete.mockResolvedValue(undefined);
    mockConfirmModal.mockReturnValue({ close: vi.fn(), destroy: vi.fn() });
    const group = await renderTab();

    await user.click(group.getByRole("button", { name: "Actions for Bug" }));
    await user.click(await screen.findByRole("menuitem", { name: /Delete/ }));
    // Opening the confirmation is not a delete.
    expect(mockDelete).not.toHaveBeenCalled();

    const config = mockConfirmModal.mock.calls.at(-1)?.[0] as {
      title: string;
      content: string;
      okButtonProps: { danger?: boolean };
      onOk: () => Promise<void>;
    };
    expect(config.title).toBe("Delete label?");
    expect(config.content).toContain("Bug");
    expect(config.okButtonProps).toEqual({ danger: true });

    await config.onOk();
    expect(mockDelete).toHaveBeenCalledWith({
      id: "label-1",
      resource_type: "issue",
    });
  });

  /**
   * The rethrow contract, asserted on the config rather than on the DOM.
   *
   * `confirmModal` closes on the line after `onOk` unless `onOk` returns a
   * promise, and stays open when that promise rejects — so a delete that fails
   * must leave the confirmation up rather than dismiss as though the label were
   * gone. Two DOM-level spellings of this were tried and **both were vacuous**,
   * verified by breaking the code they were meant to guard:
   *
   *   - `expect(screen.getByText("Delete label?")).toBeInTheDocument()` stayed
   *     green with `onConfirm` swallowing the rejection — the popup node
   *     outlives the close, so "the title is in the document" is true either
   *     way. `tokens-tab.test.tsx` asserts exactly this and has the same hole.
   *   - `expect(screen.getByRole("dialog")).toHaveAttribute("data-open")`, with
   *     a settle delay first, also stayed green: under jsdom the exit never
   *     completes, so the closing dialog keeps `data-open` forever.
   *
   * What the promise did is only observable at the promise. `onOk` is the whole
   * contract, so the test reads it off the config and awaits it.
   */
  it("hands confirmModal an onOk that rejects when the request fails", async () => {
    const user = userEvent.setup();
    labelsRef.current = [LABEL];
    mockDelete.mockRejectedValue(new Error("network down"));
    mockConfirmModal.mockReturnValue({ close: vi.fn(), destroy: vi.fn() });
    const group = await renderTab();

    await user.click(group.getByRole("button", { name: "Actions for Bug" }));
    await user.click(await screen.findByRole("menuitem", { name: /Delete/ }));

    const config = mockConfirmModal.mock.calls.at(-1)?.[0] as {
      onOk: () => Promise<void>;
    };
    expect(config).toBeDefined();
    await expect(config.onOk()).rejects.toThrow("network down");
    expect(mockToastError).toHaveBeenCalledWith("network down");
  });
});
