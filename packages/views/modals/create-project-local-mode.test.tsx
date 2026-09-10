// @vitest-environment jsdom

import React from "react";
import { describe, it, expect, vi, beforeEach } from "vitest";
import { fireEvent, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { renderWithI18n } from "../test/i18n";

// Runtime list drives the worktree version gate. Each test sets the reported
// CLI version before rendering.
let runtimeCliVersion = "9.9.9";
// What the fake runtime row says about worktree support. This — not the version
// string — is what the gate reads now (MUL-5707), and the three values are
// genuinely different states: "yes", "a daemon that cannot", and a row written
// by a server too old to record capabilities at all (#7113).
let runtimeWorktreeMetadata: "advertised" | "daemon_cannot" | "server_recorded_nothing" =
  "advertised";
// What the desktop validator reports for the picked folder.
let pickedIsGitRepo: boolean | undefined = true;
let pickedRemotes: Array<{ name: string; url: string }> = [];

const createProjectMock = vi.fn().mockResolvedValue({ id: "p1", slug: "p1" });

// jsdom has no IntersectionObserver; the modal's scroll affordances construct
// one, and the resulting rejection aborts the submit handler mid-flight.
class NoopIntersectionObserver {
  observe() {}
  unobserve() {}
  disconnect() {}
  takeRecords() {
    return [];
  }
}
vi.stubGlobal("IntersectionObserver", NoopIntersectionObserver);

vi.mock("@tanstack/react-query", () => ({
  // Query-aware: the modal issues several. Only the runtime list matters here;
  // returning runtimes for all of them would feed them to the member/agent
  // pickers, which expect a different shape.
  useQuery: (options: { queryKey?: unknown[] }) => {
    const key = options?.queryKey?.[0];
    if (key === "members" || key === "agents") return { data: [] };
    return {
      data: [
        {
          daemon_id: "daemon-1",
          metadata: {
            cli_version: runtimeCliVersion,
            // A capability-aware server always writes the key — null when the
            // daemon sent no header — so its absence means the SERVER is old.
            ...(runtimeWorktreeMetadata === "advertised"
              ? { capabilities: ["local-worktree-v1", "local-worktree-committed-base-v1"] }
              : runtimeWorktreeMetadata === "daemon_cannot"
                ? { capabilities: ["skill-bundles-v1"] }
                : {}),
          },
        },
      ],
    };
  },
  // runtimeListOptions builds its descriptor with this; the mocked useQuery
  // reads queryKey off it, so an identity passthrough is enough.
  queryOptions: (options: unknown) => options,
}));

vi.mock("@orvilo/core/projects/mutations", () => ({
  useCreateProject: () => ({ mutateAsync: createProjectMock }),
}));

vi.mock("@orvilo/core/projects", () => ({
  useProjectDraftStore: (selector: (state: unknown) => unknown) =>
    selector({
      draft: {
        title: "Game Client",
        description: "",
        status: "planned",
        priority: "none",
        icon: "📁",
        leadId: null,
        leadType: null,
        startDate: null,
        dueDate: null,
        repos: [],
      },
      setDraft: vi.fn(),
      clearDraft: vi.fn(),
      resetDraft: vi.fn(),
    }),
}));

// Whether the connected server validates execution_mode at all. Absent on every
// release before the worktree save gate.
let serverValidatesWorktree = true;
let serverSupportsCommittedBase = true;
vi.mock("@orvilo/core/config", () => ({
  useConfigStore: (selector: (state: { localWorktreeSupported: boolean; localWorktreeCommittedBaseSupported: boolean }) => unknown) =>
    selector({ localWorktreeSupported: serverValidatesWorktree, localWorktreeCommittedBaseSupported: serverSupportsCommittedBase }),
}));

vi.mock("@orvilo/core/hooks", () => ({ useWorkspaceId: () => "workspace-1" }));
vi.mock("@orvilo/core/paths", () => ({
  useCurrentWorkspace: () => ({ id: "workspace-1", slug: "ws", repos: [] }),
  useWorkspacePaths: () => ({ projectDetail: (id: string) => `/ws/projects/${id}` }),
}));
vi.mock("@orvilo/core/workspace/queries", () => ({
  memberListOptions: () => ({ queryKey: ["members"], queryFn: vi.fn() }),
  agentListOptions: () => ({ queryKey: ["agents"], queryFn: vi.fn() }),
}));
vi.mock("@orvilo/core/workspace/hooks", () => ({
  useActorName: () => ({ getActorName: vi.fn() }),
}));
vi.mock("../navigation", () => ({ useNavigation: () => ({ push: vi.fn() }) }));
vi.mock("../editor", () => {
  const ContentEditor = React.forwardRef<{ getMarkdown: () => string }, { placeholder?: string }>(
    ({ placeholder }, ref) => {
      React.useImperativeHandle(ref, () => ({ getMarkdown: () => "" }));
      return <textarea placeholder={placeholder} />;
    },
  );
  ContentEditor.displayName = "ContentEditor";
  return {
    ContentEditor,
    TitleEditor: ({
      placeholder,
      onChange,
    }: {
      placeholder?: string;
      onChange?: (value: string) => void;
    }) => <input placeholder={placeholder} onChange={(e) => onChange?.(e.target.value)} />,
  };
});
vi.mock("../issues/components/priority-icon", () => ({ PriorityIcon: () => <span /> }));
vi.mock("../common/actor-avatar", () => ({ ActorAvatar: () => <span /> }));
vi.mock("../projects/components/project-start-date-picker", () => ({
  ProjectStartDatePicker: () => <button type="button">Start date</button>,
}));
vi.mock("../projects/components/project-due-date-picker", () => ({
  ProjectDueDatePicker: () => <button type="button">Due date</button>,
}));

// Desktop folder selection discovers the repository from its actual remotes.
vi.mock("../platform/local-directory", () => ({
  isDesktopShell: () => true,
  pickDirectory: () =>
    Promise.resolve({ ok: true, path: "/Users/dev/work/game-client", basename: "game-client" }),
  validateLocalDirectory: () => Promise.resolve({ ok: true, is_git_repo: pickedIsGitRepo, remotes: pickedRemotes }),
}));
vi.mock("../platform/use-local-daemon-status", () => ({
  useLocalDaemonStatus: () => ({ daemonId: "daemon-1", deviceName: "MacBook", running: true }),
}));

// Render overlays inline so their contents are assertable.
vi.mock("@orvilo/ui/components/ui/dialog", () => ({
  Dialog: ({ children }: { children: React.ReactNode }) => <div>{children}</div>,
  DialogContent: ({ children }: { children: React.ReactNode }) => <div>{children}</div>,
  DialogTitle: ({ children }: { children: React.ReactNode }) => <div>{children}</div>,
}));
vi.mock("@orvilo/ui/components/ui/popover", () => ({
  Popover: ({ children }: { children: React.ReactNode }) => <>{children}</>,
  PopoverTrigger: ({ render }: { render: React.ReactNode }) => <>{render}</>,
  PopoverContent: ({ children }: { children: React.ReactNode }) => <div>{children}</div>,
}));
vi.mock("@orvilo/ui/components/ui/dropdown-menu", () => ({
  DropdownMenu: ({ children }: { children: React.ReactNode }) => <>{children}</>,
  DropdownMenuTrigger: ({ render }: { render: React.ReactNode }) => <>{render}</>,
  DropdownMenuContent: ({ children }: { children: React.ReactNode }) => <>{children}</>,
  DropdownMenuItem: ({ children, onClick }: { children: React.ReactNode; onClick?: () => void }) => (
    <button type="button" onClick={onClick}>
      {children}
    </button>
  ),
}));
vi.mock("sonner", () => ({ toast: { success: vi.fn(), error: vi.fn() } }));

import { CreateProjectModal, buildLocalDirectoryResourceRef } from "./create-project";

async function pickLocalDirectory(user: ReturnType<typeof userEvent.setup>) {
  await user.click(screen.getByRole("button", { name: /Add folder/i }));
  await waitFor(() => expect(screen.getAllByText("game-client").length).toBeGreaterThan(0));
}

describe("CreateProjectModal — local directory execution mode", () => {
  beforeEach(() => {
    createProjectMock.mockClear();
    runtimeCliVersion = "9.9.9";
    runtimeWorktreeMetadata = "advertised";
    serverValidatesWorktree = true;
    serverSupportsCommittedBase = true;
    pickedIsGitRepo = true;
    pickedRemotes = [];
  });


  it("creates the project with the selected folder and its detected origin", async () => {
    pickedRemotes = [
      { name: "upstream", url: "https://github.com/upstream/game-client.git" },
      { name: "origin", url: "https://github.com/dev/game-client.git" },
    ];
    const user = userEvent.setup();
    renderWithI18n(<CreateProjectModal onClose={vi.fn()} />);

    expect(screen.queryByRole("button", { name: "GitHub repos" })).not.toBeInTheDocument();
    expect(screen.queryByPlaceholderText(/github.com\/owner/)).not.toBeInTheDocument();
    await pickLocalDirectory(user);
    expect(screen.getByText("dev/game-client")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: /Create project/i }));
    await waitFor(() => expect(createProjectMock).toHaveBeenCalledWith(expect.objectContaining({
      resources: [
        { resource_type: "github_repo", resource_ref: { url: "https://github.com/dev/game-client.git" } },
        { resource_type: "local_directory", resource_ref: expect.objectContaining({ local_path: "/Users/dev/work/game-client", daemon_id: "daemon-1" }) },
      ],
    })));
  });

  it("does not submit when Enter accepts an IME candidate", () => {
    renderWithI18n(<CreateProjectModal onClose={vi.fn()} />);
    const input = screen.getByRole("textbox", { name: /Project title/i });
    fireEvent.keyDown(input, { key: "Enter", isComposing: true });
    expect(createProjectMock).not.toHaveBeenCalled();
    fireEvent.keyDown(input, { key: "Enter" });
    expect(createProjectMock).toHaveBeenCalledTimes(1);
  });

  it("uses worktree without exposing a redundant mode or path", async () => {
    const user = userEvent.setup();
    renderWithI18n(<CreateProjectModal onClose={vi.fn()} />);
    await pickLocalDirectory(user);
    expect(screen.queryByText("/Users/dev/work/game-client")).not.toBeInTheDocument();
    expect(screen.queryByRole("radio")).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /Change directory/i })).not.toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: /Create project/i }));
    expect(createProjectMock).toHaveBeenCalledWith(expect.objectContaining({ resources: [expect.objectContaining({resource_ref: expect.objectContaining({ execution_mode: "worktree" })})] }));
  });

  it("requires explicit direct editing for a plain folder", async () => {
    pickedIsGitRepo = false;
    const user = userEvent.setup();
    renderWithI18n(<CreateProjectModal onClose={vi.fn()} />);
    await pickLocalDirectory(user);
    expect(screen.getByRole("button", { name: /Create project/i })).toBeDisabled();
    await user.click(screen.getByRole("radio", { name: /Edit this folder directly/i }));
    await user.click(screen.getByRole("button", { name: /Create project/i }));
    expect(createProjectMock).toHaveBeenCalledWith(expect.objectContaining({ resources: [expect.objectContaining({resource_ref: expect.objectContaining({ execution_mode: "in_place" })})] }));
  });

  it("removing a folder also clears its discovered remote", async () => {
    pickedRemotes = [{name: "origin", url: "https://github.com/dev/game-client.git"}];
    const user = userEvent.setup();
    renderWithI18n(<CreateProjectModal onClose={vi.fn()} />);
    await pickLocalDirectory(user);
    await user.click(screen.getByRole("button", { name: /^Clear$/i }));
    expect(screen.queryByText("dev/game-client")).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: /Add folder/i })).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: /Create project/i }));
    expect(createProjectMock).toHaveBeenCalledWith(expect.objectContaining({ resources: undefined }));
  });

  it("blocks worktree when the server cannot honour it", async () => {
    serverValidatesWorktree = false;
    const user = userEvent.setup();
    renderWithI18n(<CreateProjectModal onClose={vi.fn()} />);
    await pickLocalDirectory(user);
    expect(screen.getByRole("radio", { name: /Run in parallel, isolated/i })).toBeDisabled();
    expect(screen.getByRole("button", { name: /Create project/i })).toBeDisabled();
  });

});

// The payload is what the server stores and the daemon later reads; a missing
// or wrong execution_mode is invisible until a task actually runs. Asserted on
// the builder directly rather than through a full modal submit, which drags in
// unrelated modal machinery without testing anything more about this contract.
describe("buildLocalDirectoryResourceRef", () => {
  it("carries the chosen mode", () => {
    expect(
      buildLocalDirectoryResourceRef({
        localPath: "/Users/dev/work/game-client",
        daemonId: "daemon-1",
        label: "game-client",
        mode: "worktree",
        committedBaseSupported: true,
      }),
    ).toEqual({
      local_path: "/Users/dev/work/game-client",
      daemon_id: "daemon-1",
      label: "game-client",
      execution_mode: "worktree",
      worktree_base: "head",
    });
  });

  it("defaults are explicit: in_place is sent, not omitted", () => {
    // The server treats an absent field as in_place, but sending it keeps the
    // stored ref self-describing for anyone reading it later.
    expect(
      buildLocalDirectoryResourceRef({
        localPath: "/tmp/x",
        daemonId: "d",
        label: null,
        mode: "in_place",
      }),
    ).toEqual({ local_path: "/tmp/x", daemon_id: "d", execution_mode: "in_place" });
  });
});
