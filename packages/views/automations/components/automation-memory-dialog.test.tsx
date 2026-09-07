import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { fireEvent, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { renderWithI18n } from "../../test/i18n";
import { ApiError } from "@patchbay/core/api/client";

const mocks = vi.hoisted(() => ({
  list: vi.fn(),
  detail: vi.fn(),
  update: vi.fn(),
  remove: vi.fn(),
  toastSuccess: vi.fn(),
  toastError: vi.fn(),
}));

vi.mock("@patchbay/core/automations", async () => {
  const actual = await vi.importActual<typeof import("@tanstack/react-query")>("@tanstack/react-query");
  return {
  automationMemoryKeys: {
    list: (wsId: string, automationId: string) => ["memory-list", wsId, automationId],
    detail: (wsId: string, automationId: string, name: string) => ["memory", wsId, automationId, name],
  },
  automationMemoryListOptions: (wsId: string, automationId: string) => ({
    queryKey: ["memory-list", wsId, automationId],
    queryFn: () => mocks.list(),
  }),
  automationMemoryOptions: (wsId: string, automationId: string, name: string, options?: { enabled?: boolean }) => ({
    queryKey: ["memory", wsId, automationId, name],
    queryFn: () => mocks.detail(name),
    enabled: options?.enabled ?? true,
  }),
    useUpdateAutomationMemory: () => actual.useMutation({ mutationFn: mocks.update }),
    useDeleteAutomationMemory: () => actual.useMutation({ mutationFn: mocks.remove }),
  };
});

vi.mock("@patchbay/core/hooks", () => ({
  useWorkspaceId: () => "workspace-1",
}));

vi.mock("sonner", () => ({
  toast: { success: mocks.toastSuccess, error: mocks.toastError },
}));

import { AutomationMemoryDialog } from "./automation-memory-dialog";

function renderDialog() {
  const queryClient = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return renderWithI18n(
    <QueryClientProvider client={queryClient}>
      <AutomationMemoryDialog automationId="automation-1" onOpenChange={vi.fn()} />
    </QueryClientProvider>,
  );
}

describe("AutomationMemoryDialog", () => {
  beforeEach(() => {
    mocks.list.mockReset();
    mocks.detail.mockReset();
    mocks.update.mockReset();
    mocks.remove.mockReset();
    mocks.toastSuccess.mockReset();
    mocks.toastError.mockReset();
  });

  it("saves the virtual default file with expected revision zero", async () => {
    mocks.list.mockResolvedValue({ items: [] });
    mocks.update.mockResolvedValue({
      name: "MEMORIES.md",
      content: "Remember the release window.",
      revision: 1,
      updated_at: "2026-09-06T20:00:00Z",
    });
    const user = userEvent.setup();
    renderDialog();

    const dialog = await screen.findByRole("dialog", { name: "Memory Notes" });
    expect(await within(dialog).findByRole("combobox", { name: "File" })).toHaveTextContent("MEMORIES.md");
    expect(mocks.detail).not.toHaveBeenCalled();
    fireEvent.change(within(dialog).getByRole("textbox", { name: "Memory content" }), {
      target: { value: "Remember the release window." },
    });
    await user.click(within(dialog).getByRole("button", { name: "Save" }));

    expect(mocks.update).toHaveBeenCalledWith({
      automationId: "automation-1",
      name: "MEMORIES.md",
      content: "Remember the release window.",
      expected_revision: 0,
    }, expect.any(Object));
    expect(mocks.toastSuccess).toHaveBeenCalledWith("Memory file saved");
  });

  it("keeps a conflicting draft and does not show false success", async () => {
    mocks.list
      .mockResolvedValueOnce({
        items: [{ name: "MEMORIES.md", revision: 2, updated_at: "2026-09-06T20:00:00Z" }],
      })
      .mockResolvedValue({
        items: [{ name: "MEMORIES.md", revision: 3, updated_at: "2026-09-06T20:05:00Z" }],
      });
    mocks.detail
      .mockResolvedValueOnce({
        name: "MEMORIES.md",
        content: "Server content",
        revision: 2,
        updated_at: "2026-09-06T20:00:00Z",
      })
      .mockResolvedValue({
        name: "MEMORIES.md",
        content: "Latest server content",
        revision: 3,
        updated_at: "2026-09-06T20:05:00Z",
      });
    mocks.update.mockRejectedValue(new ApiError("memory revision conflict", 409, "Conflict"));
    const user = userEvent.setup();
    renderDialog();

    const editor = await screen.findByRole("textbox", { name: "Memory content" });
    await waitFor(() => expect(editor).toHaveValue("Server content"));
    fireEvent.change(editor, { target: { value: "Unsaved local draft" } });
    await user.click(screen.getByRole("button", { name: "Save" }));

    await waitFor(() => expect(mocks.toastError).toHaveBeenCalledWith("memory revision conflict"));
    expect(editor).toHaveValue("Unsaved local draft");
    expect(mocks.toastSuccess).not.toHaveBeenCalled();
    expect(await screen.findByRole("alert")).toHaveTextContent("This file changed elsewhere");
    await waitFor(() => {
      expect(mocks.list).toHaveBeenCalledTimes(2);
      expect(mocks.detail).toHaveBeenCalledTimes(2);
    });

    await user.click(screen.getByRole("button", { name: "Reset" }));
    expect(editor).toHaveValue("Latest server content");
    fireEvent.change(editor, { target: { value: "Rebased draft" } });
    expect(screen.getByRole("button", { name: "Save" })).toBeEnabled();
  });

  it("disables save while the newly selected file is loading", async () => {
    let resolveSecond!: (value: unknown) => void;
    mocks.list.mockResolvedValue({
      items: [
        { name: "MEMORIES.md", revision: 2, updated_at: "2026-09-06T20:00:00Z" },
        { name: "PROJECT.md", revision: 4, updated_at: "2026-09-06T21:00:00Z" },
      ],
    });
    mocks.detail.mockImplementation((name: string) => name === "MEMORIES.md"
      ? Promise.resolve({ name, content: "First", revision: 2, updated_at: "2026-09-06T20:00:00Z" })
      : new Promise((resolve) => { resolveSecond = resolve; }));
    const user = userEvent.setup();
    renderDialog();

    expect(await screen.findByRole("textbox", { name: "Memory content" })).toHaveValue("First");
    await user.click(screen.getByRole("combobox", { name: "File" }));
    await user.click(await screen.findByRole("option", { name: "PROJECT.md" }));
    expect(screen.getByRole("button", { name: "Save" })).toBeDisabled();

    resolveSecond({
      name: "PROJECT.md",
      content: "Second",
      revision: 4,
      updated_at: "2026-09-06T21:00:00Z",
    });
    await waitFor(() => expect(screen.getByRole("textbox", { name: "Memory content" })).toHaveValue("Second"));
  });

  it("uses the loaded file revision when the list summary is stale", async () => {
    mocks.list.mockResolvedValue({
      items: [{ name: "MEMORIES.md", revision: 2, updated_at: "2026-09-06T20:00:00Z" }],
    });
    mocks.detail.mockResolvedValue({
      name: "MEMORIES.md",
      content: "Newer server content",
      revision: 3,
      updated_at: "2026-09-06T20:05:00Z",
    });
    mocks.update.mockResolvedValue({
      name: "MEMORIES.md",
      content: "Edited newer content",
      revision: 4,
      updated_at: "2026-09-06T20:06:00Z",
    });
    const user = userEvent.setup();
    renderDialog();

    const editor = await screen.findByRole("textbox", { name: "Memory content" });
    await waitFor(() => expect(editor).toHaveValue("Newer server content"));
    fireEvent.change(editor, { target: { value: "Edited newer content" } });
    expect(screen.getByRole("button", { name: "Save" })).toBeEnabled();
    await user.click(screen.getByRole("button", { name: "Save" }));

    expect(mocks.update).toHaveBeenCalledWith(expect.objectContaining({
      name: "MEMORIES.md",
      expected_revision: 3,
    }), expect.any(Object));
  });

  it("recovers the latest revision after a delete conflict and retains the draft", async () => {
    mocks.list
      .mockResolvedValueOnce({
        items: [{ name: "MEMORIES.md", revision: 2, updated_at: "2026-09-06T20:00:00Z" }],
      })
      .mockResolvedValue({
        items: [{ name: "MEMORIES.md", revision: 3, updated_at: "2026-09-06T20:05:00Z" }],
      });
    mocks.detail
      .mockResolvedValueOnce({
        name: "MEMORIES.md",
        content: "Server content",
        revision: 2,
        updated_at: "2026-09-06T20:00:00Z",
      })
      .mockResolvedValue({
        name: "MEMORIES.md",
        content: "Latest server content",
        revision: 3,
        updated_at: "2026-09-06T20:05:00Z",
      });
    mocks.remove
      .mockRejectedValueOnce(new ApiError("memory revision conflict", 409, "Conflict"))
      .mockResolvedValueOnce(undefined);
    const user = userEvent.setup();
    renderDialog();

    const editor = await screen.findByRole("textbox", { name: "Memory content" });
    await waitFor(() => expect(editor).toHaveValue("Server content"));
    fireEvent.change(editor, { target: { value: "Unsaved local draft" } });
    await user.click(screen.getByRole("button", { name: "Delete" }));

    expect(await screen.findByRole("alert")).toHaveTextContent("This file changed elsewhere");
    expect(editor).toHaveValue("Unsaved local draft");
    await waitFor(() => expect(mocks.detail).toHaveBeenCalledTimes(2));
    await user.click(screen.getByRole("button", { name: "Delete" }));

    expect(mocks.remove).toHaveBeenNthCalledWith(1, expect.objectContaining({ revision: 2 }), expect.any(Object));
    expect(mocks.remove).toHaveBeenNthCalledWith(2, expect.objectContaining({ revision: 3 }), expect.any(Object));
  });

  it("keeps Reset blocked and exposes Retry when conflict recovery cannot reload detail", async () => {
    mocks.list.mockResolvedValue({
      items: [{ name: "MEMORIES.md", revision: 3, updated_at: "2026-09-06T20:05:00Z" }],
    });
    mocks.detail
      .mockResolvedValueOnce({
        name: "MEMORIES.md",
        content: "Server content",
        revision: 2,
        updated_at: "2026-09-06T20:00:00Z",
      })
      .mockRejectedValueOnce(new Error("reload failed"))
      .mockResolvedValue({
        name: "MEMORIES.md",
        content: "Latest server content",
        revision: 3,
        updated_at: "2026-09-06T20:05:00Z",
      });
    mocks.update.mockRejectedValue(new ApiError("memory revision conflict", 409, "Conflict"));
    const user = userEvent.setup();
    renderDialog();

    const editor = await screen.findByRole("textbox", { name: "Memory content" });
    await waitFor(() => expect(editor).toHaveValue("Server content"));
    fireEvent.change(editor, { target: { value: "Unsaved local draft" } });
    await user.click(screen.getByRole("button", { name: "Save" }));

    expect(await screen.findByRole("button", { name: "Retry" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Reset" })).toBeDisabled();
    expect(editor).toHaveValue("Unsaved local draft");
    await user.click(screen.getByRole("button", { name: "Retry" }));

    await waitFor(() => expect(screen.queryByRole("button", { name: "Retry" })).not.toBeInTheDocument());
    expect(screen.getByRole("button", { name: "Reset" })).toBeEnabled();
    expect(editor).toHaveValue("Unsaved local draft");
  });
});
