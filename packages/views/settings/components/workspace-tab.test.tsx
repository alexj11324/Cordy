import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

const mockUpdateWorkspace = vi.hoisted(() => vi.fn());
const mockInvalidateQueries = vi.hoisted(() => vi.fn());
const mockToastSuccess = vi.hoisted(() => vi.fn());
const workspaceRef = vi.hoisted(() => ({
  current: {
    id: "workspace-1",
    name: "Test Workspace",
    slug: "test-workspace",
    description: "",
    context: "",
    issue_prefix: "TES",
    repos: [] as { url: string }[],
  },
}));
const membersRef = vi.hoisted(() => ({
  current: [
    { user_id: "user-1", role: "owner" as "owner" | "admin" | "member" },
  ],
}));

vi.mock("@tanstack/react-query", () => ({
  useQuery: () => ({ data: membersRef.current, isFetched: true }),
  useQueryClient: () => ({
    setQueryData: vi.fn(),
    getQueryData: vi.fn(() => []),
    invalidateQueries: mockInvalidateQueries,
  }),
}));

vi.mock("@orvilo/core/paths", () => ({
  paths: {
    workspace: (slug: string) => ({ issues: () => `/${slug}/issues` }),
  },
  useCurrentWorkspace: () => workspaceRef.current,
  useHasOnboarded: () => true,
  resolvePostAuthDestination: () => "/",
}));

vi.mock("@orvilo/core/platform", () => ({
  setCurrentWorkspace: vi.fn(),
}));

vi.mock("@orvilo/core/workspace/queries", () => ({
  memberListOptions: () => ({ queryKey: ["members"], queryFn: vi.fn() }),
  workspaceListOptions: () => ({ queryKey: ["workspaces"], queryFn: vi.fn() }),
  workspaceKeys: { list: () => ["workspaces"] },
}));

vi.mock("@orvilo/core/issues/queries", () => ({
  issueKeys: { all: (workspaceId: string) => ["issues", workspaceId] },
}));

vi.mock("@orvilo/core/workspace/mutations", () => ({
  useLeaveWorkspace: () => ({ mutateAsync: vi.fn() }),
  useDeleteWorkspace: () => ({ mutateAsync: vi.fn() }),
}));

vi.mock("@orvilo/core/api", () => ({
  api: {
    updateWorkspace: mockUpdateWorkspace,
    getBaseUrl: () => "http://127.0.0.1:8080",
  },
}));

vi.mock("@orvilo/core/auth", () => {
  const useAuthStore = Object.assign(
    (selector?: (state: { user: { id: string } }) => unknown) =>
      selector
        ? selector({ user: { id: "user-1" } })
        : { user: { id: "user-1" } },
    { getState: () => ({ user: { id: "user-1" } }) },
  );
  return { useAuthStore };
});

vi.mock("../../navigation", () => ({
  useNavigation: () => ({
    push: vi.fn(),
    getShareableUrl: (path: string) => `https://app.example${path}`,
  }),
}));

vi.mock("./delete-workspace-dialog", () => ({
  DeleteWorkspaceDialog: () => null,
}));

vi.mock("sonner", () => ({
  toast: { success: mockToastSuccess, error: vi.fn() },
}));

import { renderWithI18n } from "../../test/i18n";
import { WorkspaceTab } from "./workspace-tab";

// Every mount here builds Lobe form rows and two ReUI frames. Under full-suite
// parallelism that is seconds rather than milliseconds (the same reason
// `vitest.config.ts` raises the global budget for antd).
vi.setConfig({ testTimeout: 60_000, hookTimeout: 30_000 });

/**
 * `lobe: true` loads the theme bridge on demand — which is also what mounts
 * the `ModalHost` the prefix confirmation renders into — so the first query in
 * every test has to be an async one. This helper is that first query.
 *
 * The rows are reached through the `SettingsSaveState` status region's card —
 * the tab renders no `SettingsGroup`, because its chrome is ReUI's `Frame` by
 * decision 11 — so the scope here is the card that holds the name field rather
 * than a named group.
 */
async function renderTab() {
  renderWithI18n(<WorkspaceTab />, { lobe: true });
  const nameField = await screen.findByLabelText("Name");
  return within(nameField.closest('[data-slot="frame-panel"]') as HTMLElement);
}

describe("WorkspaceTab — automatic updates", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    vi.useFakeTimers({ shouldAdvanceTime: true });
    workspaceRef.current = {
      id: "workspace-1",
      name: "Test Workspace",
      slug: "test-workspace",
      description: "",
      context: "",
      issue_prefix: "TES",
      repos: [],
    };
    membersRef.current = [{ user_id: "user-1", role: "owner" }];
    mockUpdateWorkspace.mockImplementation(
      async (_id: string, payload: Record<string, unknown>) => ({
        ...workspaceRef.current,
        ...payload,
        issue_prefix:
          (payload.issue_prefix as string | undefined) ??
          workspaceRef.current.issue_prefix,
      }),
    );
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  function setupUser() {
    return userEvent.setup({ advanceTimers: vi.advanceTimersByTime });
  }

  it("renders the current prefix in the shared input control", async () => {
    const card = await renderTab();
    const input = card.getByPlaceholderText("TES") as HTMLInputElement;
    expect(input.value).toBe("TES");
  });

  it("renders the real workspace URL in a shared read-only input control", async () => {
    const card = await renderTab();

    const input = card.getByRole("textbox", {
      name: "URL",
    }) as HTMLInputElement;
    expect(input.value).toBe("https://app.example/test-workspace/issues");
    expect(input.readOnly).toBe(true);
  });

  it("keeps only the supported workspace settings controls", async () => {
    const card = await renderTab();

    expect(card.getByLabelText("Name")).toHaveAttribute("id", "workspace-name");
    expect(card.getByLabelText("URL")).toHaveAttribute("id", "workspace-url");
    expect(card.getByLabelText("Issue prefix")).toHaveAttribute(
      "id",
      "workspace-issue-prefix",
    );
    expect(card.queryByRole("textbox", { name: "Description" })).toBeNull();
    expect(card.queryByRole("textbox", { name: "Context" })).toBeNull();
    expect(card.queryByRole("textbox", { name: "Slug" })).toBeNull();
    expect(card.queryByRole("button", { name: "Leave workspace" })).toBeNull();
  });

  it("uppercases and strips non-alphanumeric prefix input", async () => {
    const user = setupUser();
    const card = await renderTab();
    const input = card.getByPlaceholderText("TES") as HTMLInputElement;

    await user.clear(input);
    await user.type(input, "ab-12!cd");

    expect(input.value).toBe("AB12CD");
  });

  it("auto-saves ordinary workspace fields without invalidating issue caches", async () => {
    const user = setupUser();
    const card = await renderTab();
    const nameInput = card.getByDisplayValue("Test Workspace");

    await user.clear(nameInput);
    await user.type(nameInput, "Renamed Workspace");
    await user.tab();

    await waitFor(() => {
      expect(mockUpdateWorkspace).toHaveBeenCalledWith("workspace-1", {
        name: "Renamed Workspace",
      });
      expect(mockToastSuccess).toHaveBeenCalledWith(
        "Workspace settings saved",
        { id: "settings-auto-save" },
      );
    });
    expect(mockInvalidateQueries).not.toHaveBeenCalled();
  });

  it("asks for confirmation on prefix blur and persists only after confirmation", async () => {
    const user = setupUser();
    const card = await renderTab();
    const input = card.getByPlaceholderText("TES") as HTMLInputElement;

    await user.clear(input);
    await user.type(input, "NEW");
    await user.tab();

    expect(mockUpdateWorkspace).not.toHaveBeenCalled();
    // The confirmation is `confirmModal`, which renders into the bridge's
    // `ModalHost` — i.e. outside this tab's tree, so it is queried from the
    // document rather than from `card`.
    await screen.findByText(/Change issue prefix/i);
    expect(screen.getByText(/TES-N/)).toBeTruthy();
    expect(screen.getByText(/NEW-N/)).toBeTruthy();

    await user.click(screen.getByRole("button", { name: "Confirm" }));

    await waitFor(() => {
      expect(mockUpdateWorkspace).toHaveBeenCalledWith("workspace-1", {
        issue_prefix: "NEW",
      });
    });
    expect(mockInvalidateQueries).toHaveBeenCalledWith({
      queryKey: ["issues", "workspace-1"],
    });
    expect(mockToastSuccess).toHaveBeenCalledWith("Workspace settings saved", {
      id: "settings-auto-save",
    });
  });

  it("does not persist a prefix when the confirmation is cancelled", async () => {
    const user = setupUser();
    const card = await renderTab();
    const input = card.getByPlaceholderText("TES") as HTMLInputElement;

    await user.clear(input);
    await user.type(input, "NEW");
    await user.tab();
    await screen.findByText(/Change issue prefix/i);
    await user.click(screen.getByRole("button", { name: "Cancel" }));

    expect(mockUpdateWorkspace).not.toHaveBeenCalled();
    expect(input.value).toBe("NEW");
  });

  it("marks an empty prefix invalid and does not persist it", async () => {
    const user = setupUser();
    const card = await renderTab();
    const input = card.getByPlaceholderText("TES") as HTMLInputElement;

    await user.clear(input);
    await user.tab();

    expect(input).toHaveAttribute("aria-invalid", "true");
    expect(mockUpdateWorkspace).not.toHaveBeenCalled();
  });

  /**
   * The three rows a regular member may not edit are dimmed as well as
   * disabled, and the URL row is not — it is read-only for everyone and never
   * dimmed. The dimming itself is a rule in `base.css` and is measured in the
   * renderer; what this file owns is which rows carry the marker.
   */
  it("disables editable workspace controls for regular members", async () => {
    membersRef.current = [{ user_id: "user-1", role: "member" }];
    const card = await renderTab();

    const prefix = card.getByPlaceholderText("TES");
    const name = card.getByDisplayValue("Test Workspace");
    expect(prefix).toBeDisabled();
    expect(name).toBeDisabled();

    const marked = (el: HTMLElement) =>
      el.closest(".orvilo-settings-row")?.className ?? "";
    expect(marked(prefix)).toContain("orvilo-settings-row-disabled");
    expect(marked(name)).toContain("orvilo-settings-row-disabled");
    // The logo row has no labelled control, so it is reached through the
    // upload control's own accessible name.
    const logoRow = card
      .getByLabelText("Change workspace logo")
      .closest(".orvilo-settings-row");
    expect(logoRow).not.toBeNull();
    expect(logoRow?.className).toContain("orvilo-settings-row-disabled");
    // `marked` returns "" when there is no row ancestor, and `expect("")
    // .not.toContain(MARK)` passes — so the negative assertion is written to
    // fail if the row was never found, rather than only if it was found
    // unmarked. Without the `not.toBe("")` this case passed for the wrong
    // reason: it kept passing when `getByLabelText("URL")` matched some
    // element outside the row grid entirely.
    const urlRow = marked(card.getByLabelText("URL") as HTMLElement);
    expect(urlRow).not.toBe("");
    expect(urlRow).not.toContain("orvilo-settings-row-disabled");
  });
});
