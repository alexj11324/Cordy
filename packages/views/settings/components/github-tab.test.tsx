import { describe, it, expect, beforeEach, vi } from "vitest";
import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { renderWithI18n } from "../../test/i18n";

const mockUpdateWorkspace = vi.hoisted(() => vi.fn());
const mockDeleteInstallation = vi.hoisted(() => vi.fn());
const mockGetConnectURL = vi.hoisted(() => vi.fn());
const mockInvalidate = vi.hoisted(() => vi.fn());
const mockNavPush = vi.hoisted(() => vi.fn());
const mockSetQueryData = vi.hoisted(() => vi.fn());
const mockToastSuccess = vi.hoisted(() => vi.fn());

const workspaceRef = vi.hoisted(() => ({
  current: {
    id: "workspace-1",
    name: "Acme",
    slug: "acme",
    settings: {} as Record<string, unknown>,
    repos: [{ url: "https://github.com/acme/api" }] as { url: string }[],
  },
}));
type MemberRole = "owner" | "admin" | "member" | "guest";
const membersRef = vi.hoisted(() => ({
  current: [{ user_id: "user-1", role: "owner" as MemberRole }],
}));
const installationsRef = vi.hoisted(() => ({
  current: {
    installations: [] as {
      id: string;
      account_login: string;
      installation_id?: number;
      connected_by?: string;
    }[],
    configured: true,
    can_manage: true as boolean,
  },
}));

vi.mock("@tanstack/react-query", () => ({
  useQuery: (opts: { queryKey: unknown[] }) => {
    const key = JSON.stringify(opts.queryKey);
    if (key.includes("members")) return { data: membersRef.current };
    if (key.includes("installations")) return { data: installationsRef.current };
    return { data: undefined };
  },
  useQueryClient: () => ({
    setQueryData: mockSetQueryData,
    invalidateQueries: mockInvalidate,
  }),
  queryOptions: <T,>(opts: T) => opts,
}));

vi.mock("@orvilo/core/hooks", () => ({
  useWorkspaceId: () => "workspace-1",
}));

vi.mock("@orvilo/core/paths", () => ({
  useCurrentWorkspace: () => workspaceRef.current,
}));

vi.mock("@orvilo/core/workspace/queries", () => ({
  memberListOptions: () => ({ queryKey: ["members"], queryFn: vi.fn() }),
  workspaceKeys: { list: () => ["workspaces"] },
}));

vi.mock("@orvilo/core/github", async () => {
  const actual =
    await vi.importActual<typeof import("@orvilo/core/github")>("@orvilo/core/github");
  return {
    ...actual,
    githubInstallationsOptions: () => ({
      queryKey: ["github", "installations"],
      queryFn: vi.fn(),
    }),
  };
});

vi.mock("@orvilo/core/api", () => ({
  api: {
    updateWorkspace: mockUpdateWorkspace,
    deleteGitHubInstallation: mockDeleteInstallation,
    getGitHubConnectURL: mockGetConnectURL,
  },
}));

vi.mock("@orvilo/core/auth", () => {
  const useAuthStore = Object.assign(
    (sel?: (s: { user: { id: string } }) => unknown) =>
      sel ? sel({ user: { id: "user-1" } }) : { user: { id: "user-1" } },
    { getState: () => ({ user: { id: "user-1" } }) },
  );
  return { useAuthStore };
});

// Mocked at the context module rather than the barrel so <AppLink> stays the
// real component and its click contract is what the test exercises.
vi.mock("../../navigation/context", () => ({
  useNavigation: () => ({
    push: mockNavPush,
    replace: vi.fn(),
    back: vi.fn(),
    pathname: "/acme/settings",
    searchParams: new URLSearchParams("tab=github"),
    hash: "",
    getShareableUrl: (p: string) => `https://app.example${p}`,
  }),
}));

vi.mock("sonner", () => ({
  toast: { success: mockToastSuccess, error: vi.fn() },
}));

import { GitHubTab } from "./github-tab";

// `{ lobe: true }` is required, not decoration: this suite used a bare `render`
// with its own `I18nProvider`, so it never reached `LobeThemeBridge` while the
// tab was on shadcn. Every control on it is Lobe now, and Lobe's `Button` and
// `Modal` throw `Please wrap your app with <ConfigProvider> (or
// <MotionProvider>)` from `useMotionComponent` without the bridge.
//
// The first query after a bridged render must be async — until the lazily
// imported bridge resolves the tree is a `Suspense` fallback of `null`.
async function renderTab() {
  const result = renderWithI18n(<GitHubTab />, { lobe: true });
  // The master switch renders on every path this tab has, so it is the handle
  // each case can wait on for the lazy bridge to resolve.
  await screen.findByRole("switch", { name: /enable github features/i });
  return result;
}

function resetFixtures() {
  vi.clearAllMocks();
  workspaceRef.current = {
    id: "workspace-1",
    name: "Acme",
    slug: "acme",
    settings: {},
    repos: [{ url: "https://github.com/acme/api" }],
  };
  membersRef.current = [{ user_id: "user-1", role: "owner" }];
  installationsRef.current = { installations: [], configured: true, can_manage: true };
}

describe("GitHubTab", () => {
  beforeEach(resetFixtures);

  it("folds the non-dev hint into the master switch description (no separate callout)", async () => {
    await renderTab();
    expect(screen.getByText(/Not a development team\? Just turn it off here\./)).toBeTruthy();
    // The old standalone callout (title + dedicated "Turn GitHub off" button) is gone.
    expect(screen.queryByRole("button", { name: /^Turn GitHub off$/ })).toBeNull();
  });

  it("does not show the hint once the master switch is off", async () => {
    workspaceRef.current.settings = { github_enabled: false };
    await renderTab();
    expect(screen.queryByText(/Not a development team\?/)).toBeNull();
  });

  it("disables every feature switch when the master switch is off", async () => {
    workspaceRef.current.settings = { github_enabled: false };
    await renderTab();

    const master = screen.getByRole("switch", { name: /enable github features/i });
    expect(master.getAttribute("aria-checked")).toBe("false");

    const switches = screen.getAllByRole("switch");
    // First switch is master; remaining must be disabled (aria-disabled or disabled attr)
    const features = switches.slice(1);
    expect(features.length).toBeGreaterThan(0);
    for (const sw of features) {
      const ariaDisabled = sw.getAttribute("aria-disabled");
      const disabled = sw.hasAttribute("disabled");
      expect(ariaDisabled === "true" || disabled).toBe(true);
    }
  });

  it("flipping the master switch off persists github_enabled=false and merges existing settings", async () => {
    const user = userEvent.setup();
    workspaceRef.current.settings = { co_authored_by_enabled: true };
    mockUpdateWorkspace.mockResolvedValue({
      ...workspaceRef.current,
      settings: { co_authored_by_enabled: true, github_enabled: false },
    });

    await renderTab();

    await user.click(screen.getByRole("switch", { name: /enable github features/i }));

    await waitFor(() => {
      expect(mockUpdateWorkspace).toHaveBeenCalledWith("workspace-1", {
        settings: { co_authored_by_enabled: true, github_enabled: false },
      });
      expect(mockToastSuccess).toHaveBeenCalledWith("Changes saved", {
        id: "settings-auto-save",
      });
    });
  });

  // The two tolerances are deliberately different, and they are not
  // interchangeable. A positive assertion's budget is free — polling returns the
  // instant the node goes — so it can be generous. A negative assertion's window
  // is spent in full when it matters, and can only ever under-report.
  /** Success path: a budget. Polling returns early, so it costs nothing. */
  const CONFIRM_CLOSE_BUDGET_MS = 8_000;
  /** Failure path: a window, spent in full. */
  const CONFIRM_OPEN_WINDOW_MS = 1_200;

  async function disconnect(user: ReturnType<typeof userEvent.setup>) {
    installationsRef.current = {
      configured: true,
      can_manage: true,
      installations: [{ id: "inst-42", account_login: "acme", installation_id: 42 }],
    };
    await renderTab();
    await user.click(screen.getByRole("button", { name: /^Disconnect$/ }));
    expect(await screen.findByText(/Orvilo will stop receiving webhooks/i)).toBeTruthy();
    // The row button and the dialog's OK button carry the same string
    // (`github.disconnect` / `github.disconnect_confirm_action` are both
    // "Disconnect"), so the confirm has to be scoped to the dialog.
    const dialog = await screen.findByRole("dialog");
    return within(dialog).getByRole("button", { name: /^Disconnect$/ });
  }

  it("clicking Disconnect opens the confirmation and only fires on confirm", async () => {
    mockDeleteInstallation.mockResolvedValue(undefined);
    const user = userEvent.setup();
    const confirmButton = await disconnect(user);

    // Clicking the row's button opened a dialog and did not call the server.
    expect(mockDeleteInstallation).not.toHaveBeenCalled();

    await user.click(confirmButton);

    await waitFor(
      () => expect(mockDeleteInstallation).toHaveBeenCalledWith("workspace-1", "inst-42"),
      { timeout: CONFIRM_CLOSE_BUDGET_MS },
    );
    // ...and the dialog leaves on success. This is what gives the negative
    // assertion below its meaning.
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull(), {
      timeout: CONFIRM_CLOSE_BUDGET_MS,
    });
  });

  // The caller half of `confirmModal`'s contract: the dialog stays open when the
  // request rejects, because `handleDisconnect` rethrows instead of swallowing.
  // Without this, a build that closes on failure passes every other assertion in
  // this file — the row exists in both states.
  it("keeps the confirmation open when the disconnect request fails", async () => {
    mockDeleteInstallation.mockRejectedValue(new Error("boom"));
    const user = userEvent.setup();
    const confirmButton = await disconnect(user);

    await user.click(confirmButton);
    await waitFor(() => expect(mockDeleteInstallation).toHaveBeenCalled());

    await new Promise((resolve) => setTimeout(resolve, CONFIRM_OPEN_WINDOW_MS));
    expect(screen.getByRole("dialog")).toBeInTheDocument();
    expect(screen.getByText(/Orvilo will stop receiving webhooks/i)).toBeTruthy();

    // Close it before the test ends. `confirmModal` pushes onto a module-level
    // stack and the popup leaves through a motion exit, so a dialog still open
    // when the tree unmounts keeps `role="dialog"` in the document long enough
    // to make the *next* test's background inert — which showed up as the
    // following case failing to find its own Disconnect button. Waiting for the
    // departure here is what keeps this case from poisoning its neighbours.
    await user.click(within(screen.getByRole("dialog")).getByRole("button", { name: "Cancel" }));
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
  });

  it("Disconnect button is still visible when the master switch is off", async () => {
    workspaceRef.current.settings = { github_enabled: false };
    installationsRef.current = {
      configured: true,
      can_manage: true,
      installations: [{ id: "inst-1", account_login: "acme", installation_id: 1 }],
    };
    await renderTab();
    expect(screen.getByRole("button", { name: /^Disconnect$/ })).toBeTruthy();
  });

  it("non-admin sees the existing connection but no Connect/Disconnect controls", async () => {
    membersRef.current = [{ user_id: "user-1", role: "member" }];
    installationsRef.current = {
      configured: true,
      can_manage: false,
      installations: [{ id: "inst-1", account_login: "acme" }],
    };
    await renderTab();

    expect(screen.getByText(/Connected to acme/i)).toBeTruthy();
    expect(screen.getByText(/Read-only view\./i)).toBeTruthy();
    expect(screen.queryByRole("button", { name: /^Connect GitHub$/ })).toBeNull();
    expect(screen.queryByRole("button", { name: /^Disconnect$/ })).toBeNull();
  });

  it("non-admin with no connection sees the contact-admin hint", async () => {
    membersRef.current = [{ user_id: "user-1", role: "member" }];
    installationsRef.current = {
      configured: true,
      can_manage: false,
      installations: [],
    };
    await renderTab();

    expect(screen.getByText(/Ask an admin or owner/i)).toBeTruthy();
    expect(screen.queryByRole("button", { name: /^Connect GitHub$/ })).toBeNull();
  });

  it("renders the connected_by line when the backend provides it", async () => {
    installationsRef.current = {
      configured: true,
      can_manage: true,
      installations: [
        {
          id: "inst-7",
          account_login: "acme",
          installation_id: 7,
          connected_by: "Jiayuan",
        },
      ],
    };
    await renderTab();
    expect(screen.getByText(/Connected by Jiayuan/)).toBeTruthy();
  });

  it("repositories shortcut navigates to the repositories tab", async () => {
    const user = userEvent.setup();
    await renderTab();
    await user.click(screen.getByRole("button", { name: /Manage repositories/ }));
    expect(mockNavPush).toHaveBeenCalledWith("/acme/settings?tab=repositories");
  });
});
