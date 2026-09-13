import { beforeEach, describe, expect, it, vi } from "vitest";
import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { renderWithI18n } from "../../test/i18n";

const mockUpdateWorkspace = vi.hoisted(() => vi.fn());
const mockGetGitHubConnectURL = vi.hoisted(() => vi.fn());
const mockFetchNextPage = vi.hoisted(() => vi.fn());
const mockNavReplace = vi.hoisted(() => vi.fn());
const mockToastSuccess = vi.hoisted(() => vi.fn());
const workspaceRef = vi.hoisted(() => ({
  current: {
    id: "workspace-1",
    name: "Test Workspace",
    slug: "test-workspace",
    repos: [{ url: "https://github.com/alexj11324/Cordy" }] as {
      url: string;
      description?: string;
    }[],
  },
}));
const membersRef = vi.hoisted(() => ({
  current: [{ user_id: "user-1", role: "owner" as "owner" | "admin" | "member" }],
}));
const githubRef = vi.hoisted(() => ({
  current: {
    installations: [] as { id: string; account_login: string }[],
    configured: true,
    repository_browse_configured: true,
    can_manage: true,
  },
}));
const githubQueryStateRef = vi.hoisted(() => ({
  current: {
    isPending: false,
    isFetching: false,
  },
}));
const githubRepositoriesRef = vi.hoisted(() => ({
  current: [] as {
    id: number;
    full_name: string;
    html_url: string;
    clone_url: string;
    description: string | null;
    private: boolean;
    archived: boolean;
    default_branch: string;
  }[],
}));
const searchParamsRef = vi.hoisted(() => ({
  current: new URLSearchParams("tab=repositories"),
}));

vi.mock("@tanstack/react-query", () => ({
  useQuery: (options: { queryKey: readonly unknown[] }) => {
    if (options.queryKey[0] === "projects") return {data: []};
    if (options.queryKey.includes("installations")) {
      return { data: githubRef.current, ...githubQueryStateRef.current };
    }
    return { data: membersRef.current };
  },
  useInfiniteQuery: () => ({
    data: {
      pages: [
        {
          repositories: githubRepositoriesRef.current,
          total_count: githubRepositoriesRef.current.length,
          next_page: null,
        },
      ],
    },
    isPending: false,
    isError: false,
    hasNextPage: false,
    isFetchingNextPage: false,
    fetchNextPage: mockFetchNextPage,
  }),
  useQueryClient: () => ({ setQueryData: vi.fn() }),
  queryOptions: <T,>(options: T) => options,
  infiniteQueryOptions: <T,>(options: T) => options,
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

vi.mock("@orvilo/core/api", () => ({
  api: {
    updateWorkspace: mockUpdateWorkspace,
    getGitHubConnectURL: mockGetGitHubConnectURL,
  },
}));

vi.mock("@orvilo/core/auth", () => {
  const useAuthStore = Object.assign(
    (selector?: (state: { user: { id: string } }) => unknown) =>
      selector ? selector({ user: { id: "user-1" } }) : { user: { id: "user-1" } },
    { getState: () => ({ user: { id: "user-1" } }) },
  );
  return { useAuthStore };
});

vi.mock("sonner", () => ({
  toast: { success: mockToastSuccess, error: vi.fn() },
}));

vi.mock("../../navigation", () => ({
  useNavigation: () => ({
    push: vi.fn(),
    replace: mockNavReplace,
    back: vi.fn(),
    pathname: "/acme/settings",
    searchParams: searchParamsRef.current,
    hash: "",
    getShareableUrl: (path: string) => `https://app.example${path}`,
  }),
}));

import { RepositoriesTab, repositoryIdentity } from "./repositories-tab";

// `{ lobe: true }` is required, not decoration: this suite used a bare `render`
// with its own `I18nProvider`, so it never reached `LobeThemeBridge` while the
// tab was on shadcn. Every control on it is Lobe now. A test file is a host
// surface, and it is the one that appears in no screenshot.
//
// Async because the bridge is lazy — until its module resolves the tree is a
// `Suspense` fallback of `null`. The "Remote sources" group renders on every
// path, which makes it the handle each case can wait on.
async function renderTab() {
  const result = renderWithI18n(<RepositoriesTab />, { lobe: true });
  // Waited on as a DOM node, not by role. One case ("opens the picker after
  // returning from a GitHub connection") mounts with a modal already open, and
  // base-ui marks everything behind a modal inert — so the group is in the
  // document but is not reachable by `getByRole` there, and a role query would
  // time out on a perfectly healthy render.
  await waitFor(() =>
    expect(document.querySelector('[role="group"]')).not.toBeNull(),
  );
  return result;
}

/**
 * Polls on a real timer, for assertions whose subject leaves through a frame
 * rather than through a commit.
 *
 * `waitFor` is not usable for the confirm dialog's departure, and the reason is
 * measured rather than assumed: with `vi.useFakeTimers({ shouldAdvanceTime:
 * true })` installed by an earlier test in this file, an 8s `waitFor` budget
 * expired **and** an 8s real-time poll expired with it, while the same
 * assertion passed the moment the file ran without fake timers anywhere. The
 * OK button's promise had settled and `close()` had run — the button was out of
 * its loading state within 10ms — but the popup kept `data-open` for the whole
 * budget. Which module captured a faked timer is not something this file
 * chased down; what it did was stop faking them, since nothing here needs the
 * debounce.
 *
 * The budget discipline is the reference's: a generous ceiling that returns the
 * instant the condition holds, so a correct build that closes slowly still
 * passes.
 */
async function waitForRealTime(assert: () => void, budgetMs = 8_000) {
  const deadline = Date.now() + budgetMs;
  for (;;) {
    try {
      assert();
      return;
    } catch (error) {
      if (Date.now() >= deadline) throw error;
    }
    await new Promise((resolve) => setTimeout(resolve, 50));
  }
}

describe("RepositoriesTab — automatic updates", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    workspaceRef.current = {
      id: "workspace-1",
      name: "Test Workspace",
      slug: "test-workspace",
      repos: [{ url: "https://github.com/alexj11324/Cordy" }],
    };
    membersRef.current = [{ user_id: "user-1", role: "owner" }];
    githubRef.current = {
      installations: [],
      configured: true,
      repository_browse_configured: true,
      can_manage: true,
    };
    githubQueryStateRef.current = {
      isPending: false,
      isFetching: false,
    };
    githubRepositoriesRef.current = [];
    searchParamsRef.current = new URLSearchParams("tab=repositories");
    mockNavReplace.mockImplementation((path: string) => {
      searchParamsRef.current = new URLSearchParams(path.split("?")[1] ?? "");
    });
    mockUpdateWorkspace.mockImplementation(
      async (_id: string, payload: { repos: { url: string; description?: string }[] }) => ({
        ...workspaceRef.current,
        repos: payload.repos,
      }),
    );
  });

  function setupUser() {
    return userEvent.setup();
  }

  it("shows repository names and details without URL or description inputs", async () => {
    await renderTab();
    expect(screen.queryAllByRole("textbox")).toHaveLength(0);
    expect(screen.getByText("Remote address")).toBeTruthy();
    expect(mockUpdateWorkspace).not.toHaveBeenCalled();
  });

  it("only adds a remote after the dialog is submitted", async () => {
    const user = setupUser();
    await renderTab();
    await user.click(screen.getByRole("button", {name: "Add a remote repository"}));
    await user.type(screen.getByRole("textbox"), "git@github.com:orvilo-ai/second.git");
    expect(mockUpdateWorkspace).not.toHaveBeenCalled();
    await user.click(screen.getAllByRole("button", {name: "Add a remote repository"}).at(-1)!);
    await waitFor(() => expect(mockUpdateWorkspace).toHaveBeenCalledWith("workspace-1", {repos: [
      {url: "https://github.com/alexj11324/Cordy"}, {url: "git@github.com:orvilo-ai/second.git"},
    ]}));
  });

  it("keeps stored descriptions without asking users to edit them", async () => {
    workspaceRef.current = {...workspaceRef.current, repos: [{url: "https://github.com/alexj11324/Cordy", description: "Main app"}]};
    await renderTab();
    expect(screen.queryAllByRole("textbox")).toHaveLength(0);
    expect(mockUpdateWorkspace).not.toHaveBeenCalled();
  });

  it("persists confirmed removal without deleting a local checkout", async () => {
    const user = setupUser();
    await renderTab();
    await user.click(screen.getByRole("button", {name: "Delete repository"}));
    // Opening the confirmation must not touch the server.
    expect(mockUpdateWorkspace).not.toHaveBeenCalled();
    // The row button and the dialog's OK button carry the same string
    // (`repositories.delete_aria` and `delete_confirm_action` are both "Delete
    // repository"), so the confirm has to be scoped to the dialog.
    const dialog = await screen.findByRole("dialog");
    await user.click(within(dialog).getByRole("button", { name: "Delete repository" }));
    await waitFor(() =>
      expect(mockUpdateWorkspace).toHaveBeenCalledWith("workspace-1", {repos: []}),
    );
    // ...and the dialog leaves once the promise `onConfirm` returned settles.
    // This is what gives the absent-dialog assertions elsewhere their meaning.
    await waitForRealTime(() =>
      expect(screen.queryByRole("dialog")).toBeNull(),
    );
  });

  // There is deliberately **no** failure-path counterpart here, and that is a
  // stated gap rather than an oversight — the mechanism is different on this
  // tab from every other confirm in the family.
  //
  // The GitHub tab's disconnect (and every `useSettingsConfirm` call site that
  // goes through `mutateAsync`) can hold its dialog open on failure, because the
  // call site returns a promise that rejects. Removal here reaches the server by
  // a different channel than this dialog: it replaces local state and hands the
  // new list to `useAutoSave.saveNow`, whose `onSave` is `saveRepositories` —
  // an `api.updateWorkspace` PATCH. What is fire-and-forget is the *promise*,
  // not the request (`void runSave(next)`, returning `undefined`), so the
  // promise `onConfirm` returns is already resolved by the time the dialog
  // tests it, exactly as the old `AlertDialogAction` closed synchronously on
  // click. A failure therefore shows up as the autosave readout flipping to its
  // error state in this group's header, plus the toast, not as a dialog that
  // stays open.
  //
  // Making it await would take a change to `use-auto-save.ts` — `flush()` saves
  // `latestValueRef.current`, which at that instant is still the pre-removal
  // list, so calling it here would save the old value back and lose the
  // removal. That is a state-model change, not a migration step.

  it("shows the autosave failure in the Remote sources header", async () => {
    // `mockRejectedValue`, not `...Once`. The removal's own `saveNow` is the
    // first call, but the hook's debounce re-saves the same list ~650ms later
    // and a `...Once` would let that second call succeed — the readout would
    // read "Saved" again before the assertion could see the error, which is a
    // guard that cannot fail.
    mockUpdateWorkspace.mockRejectedValue(new Error("boom"));
    const user = setupUser();
    await renderTab();
    await user.click(screen.getByRole("button", { name: "Delete repository" }));
    const dialog = await screen.findByRole("dialog");
    await user.click(within(dialog).getByRole("button", { name: "Delete repository" }));

    // The status region used to be `SettingsTab`'s `action`, and the dialog
    // branch returned before reading it — so inside the settings dialog this
    // readout never rendered at all. It is on this group's `extra` now, which
    // renders on both branches, and it is the only feedback this flow gives: the
    // removal is local-state + a fire-and-forget autosave, so a failure cannot
    // hold the dialog open (see the note above).
    await waitForRealTime(() => {
      const group = screen.getByRole("group", { name: "Remote sources" });
      expect(within(group).getByRole("status")).toHaveTextContent("Couldn't save");
    });
  });

  it("keeps workspace repository mutations unavailable to members", async () => {
    membersRef.current = [{user_id: "user-1", role: "member"}];
    await renderTab();
    expect(screen.queryAllByRole("textbox")).toHaveLength(0);
    expect(screen.queryByRole("button", {name: "Add a remote repository"})).toBeNull();
  });

  it("starts GitHub connection with the signed repository return target", async () => {
    const user = setupUser();
    mockGetGitHubConnectURL.mockResolvedValue({
      configured: true,
      url: "https://github.com/apps/orvilo/installations/new",
    });
    const open = vi.spyOn(window, "open").mockImplementation(() => null);
    await renderTab();

    await user.click(screen.getByRole("button", { name: "Connect GitHub" }));

    await waitFor(() => {
      expect(mockGetGitHubConnectURL).toHaveBeenCalledWith(
        "workspace-1",
        "repositories",
      );
      expect(open).toHaveBeenCalledWith(
        "https://github.com/apps/orvilo/installations/new",
        "_blank",
        "noopener",
      );
    });
    open.mockRestore();
  });

  it("keeps GitHub import disabled when repository browsing is unavailable", async () => {
    githubRef.current = {
      installations: [],
      configured: true,
      repository_browse_configured: false,
      can_manage: true,
    };
    await renderTab();

    const button = screen.getByRole("button", { name: "Connect GitHub" });
    expect(
      button.hasAttribute("disabled") ||
        button.getAttribute("aria-disabled") === "true",
    ).toBe(true);
    expect(button.getAttribute("title")).toContain("GITHUB_APP_ID");
  });

  it("imports selected GitHub repositories and deduplicates HTTPS against SSH", async () => {
    workspaceRef.current = {
      ...workspaceRef.current,
			repos: [{ url: "git@github.com:alexj11324/Cordy.git" }],
    };
    githubRef.current = {
      installations: [{ id: "installation-row-1", account_login: "orvilo-ai" }],
      configured: true,
      repository_browse_configured: true,
      can_manage: true,
    };
    githubRepositoriesRef.current = [
      {
        id: 1,
        full_name: "orvilo-ai/orvilo",
        html_url: "https://github.com/alexj11324/Cordy",
        clone_url: "https://github.com/alexj11324/Cordy.git",
        description: "Existing repository",
        private: false,
        archived: false,
        default_branch: "main",
      },
      {
        id: 2,
        full_name: "orvilo-ai/console",
        html_url: "https://github.com/orvilo-ai/console",
        clone_url: "https://github.com/orvilo-ai/console.git",
        description: "Console app",
        private: true,
        archived: false,
        default_branch: "main",
      },
    ];
    const user = setupUser();
    await renderTab();

    await user.click(
      screen.getByRole("button", { name: "Choose from GitHub" }),
    );
    const checkboxes = screen.getAllByRole("checkbox");
    expect(checkboxes).toHaveLength(2);
    expect(
      checkboxes[0]!.hasAttribute("disabled") ||
        checkboxes[0]!.getAttribute("aria-disabled") === "true",
    ).toBe(true);

    await user.click(checkboxes[1]!);
    await user.click(screen.getByRole("button", { name: "Add repositories" }));

    await waitFor(() => {
      expect(mockUpdateWorkspace).toHaveBeenCalledWith("workspace-1", {
        repos: [
					{ url: "git@github.com:alexj11324/Cordy.git" },
          {
            url: "https://github.com/orvilo-ai/console.git",
            description: "Console app",
          },
        ],
      });
    });
  });

  it("preserves repository path casing when comparing clone URLs", () => {
    expect(
      repositoryIdentity("https://GitHub.com/Acme/Repo.git"),
    ).toBe("github.com/Acme/Repo");
    expect(
      repositoryIdentity("git@github.com:acme/repo.git"),
    ).toBe("github.com/acme/repo");
  });

  it("opens the picker after returning from a GitHub connection", async () => {
    githubRef.current = {
      installations: [{ id: "installation-row-1", account_login: "orvilo-ai" }],
      configured: true,
      repository_browse_configured: true,
      can_manage: true,
    };
    searchParamsRef.current = new URLSearchParams(
      "tab=repositories&github_connected=1",
    );

    await renderTab();

    expect(
      await screen.findByRole("heading", {
        name: "Choose GitHub repositories",
      }),
    ).toBeTruthy();
    expect(mockNavReplace).toHaveBeenCalledWith(
      "/acme/settings?tab=repositories",
    );
  });

  it("clears the GitHub callback query after an empty installation result", async () => {
    searchParamsRef.current = new URLSearchParams(
      "tab=repositories&github_connected=1",
    );

    await renderTab();

    await waitFor(() => {
      expect(mockNavReplace).toHaveBeenCalledWith(
        "/acme/settings?tab=repositories",
      );
    });
    expect(
      screen.queryByRole("heading", {
        name: "Choose GitHub repositories",
      }),
    ).toBeNull();
  });
});
