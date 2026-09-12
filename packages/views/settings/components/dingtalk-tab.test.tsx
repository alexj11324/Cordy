// @vitest-environment jsdom

import { type ReactNode } from "react";
import { describe, it, expect, beforeEach, afterEach, vi } from "vitest";
import { cleanup, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { configStore } from "@orvilo/core/config";
import enSettings from "../../locales/en/settings.json";

type MemberRole = "owner" | "admin" | "member";

const membersRef = vi.hoisted(() => ({
  current: [{ user_id: "user-1", role: "owner" as MemberRole }],
}));
const agentsRef = vi.hoisted(() => ({
  current: [
    { id: "agent-1" },
    { id: "agent-7" },
    { id: "agent-8" },
  ],
}));
const installationsRef = vi.hoisted(() => ({
  current: {
    installations: [] as unknown[],
    configured: true,
    install_supported: true,
  },
}));
const groupsRef = vi.hoisted(() => ({
  current: {
    data: { groups: [], group_discovery_supported: true } as {
      groups: unknown[];
      group_discovery_supported: boolean;
      inactive_group_counts?: Record<string, number>;
      bot_identities?: Record<string, unknown>;
    },
    isLoading: false,
    isError: false,
    refetch: vi.fn(),
  },
}));
const inactiveGroupsRef = vi.hoisted(() => ({
  current: {
    data: undefined as undefined | { pages: Array<{ groups: unknown[]; next_offset: number | null }> },
    isLoading: false,
    isError: false,
    refetch: vi.fn(),
    hasNextPage: false,
    isFetchingNextPage: false,
    fetchNextPage: vi.fn(),
  },
}));
const mockRegisterBYO = vi.hoisted(() => vi.fn());
const mockDeleteInstallation = vi.hoisted(() => vi.fn());
const mockOpenExternal = vi.hoisted(() => vi.fn());
const mockInvalidate = vi.hoisted(() => vi.fn());

vi.mock("@tanstack/react-query", () => ({
  useQuery: (opts: { queryKey: unknown[]; enabled?: boolean }) => {
    if (opts.enabled === false) return { data: undefined, isLoading: false };
    const key = JSON.stringify(opts.queryKey);
    if (key.includes("members")) return { data: membersRef.current, isLoading: false };
    if (key.includes("agents")) return { data: agentsRef.current, isLoading: false };
    if (key.includes("groups")) return groupsRef.current;
    if (key.includes("installations")) return { data: installationsRef.current, isLoading: false };
    return { data: undefined, isLoading: false };
  },
  useQueryClient: () => ({ invalidateQueries: mockInvalidate }),
  useInfiniteQuery: () => inactiveGroupsRef.current,
  queryOptions: <T,>(opts: T) => opts,
}));

vi.mock("@orvilo/core/hooks", () => ({ useWorkspaceId: () => "workspace-1" }));

vi.mock("@orvilo/core/workspace/queries", () => ({
  memberListOptions: () => ({ queryKey: ["members"], queryFn: vi.fn() }),
  agentListOptions: () => ({ queryKey: ["agents"], queryFn: vi.fn() }),
}));

vi.mock("@orvilo/core/workspace/hooks", () => ({
  useActorName: () => ({
    getAgentName: (agentId: string) => `Agent ${agentId}`,
    getMemberName: () => "Unknown",
    getTeamName: () => "Unknown Team",
    getActorName: () => "Unknown",
    getActorInitials: () => "??",
    getActorAvatarUrl: () => null,
  }),
}));

vi.mock("../../common/actor-avatar", () => ({
  ActorAvatar: ({
    actorId,
    profileLink,
  }: {
    actorId: string;
    profileLink?: boolean;
  }) => (
    <span
      data-testid="actor-avatar"
      data-actor-id={actorId}
      data-profile-link={String(profileLink)}
    />
  ),
}));

vi.mock("@orvilo/core/dingtalk", () => ({
  dingtalkInstallationsOptions: () => ({
    queryKey: ["dingtalk", "installations"],
    queryFn: vi.fn(),
  }),
  dingtalkGroupsOptions: () => ({
    queryKey: ["dingtalk", "groups"],
    queryFn: vi.fn(),
  }),
  dingtalkKeys: {
    installations: (wsId: string) => ["dingtalk", "installations", wsId],
    groups: (wsId: string) => ["dingtalk", "groups", wsId],
    inactiveGroups: (wsId: string, installationId: string) =>
      ["dingtalk", "groups", wsId, "inactive", installationId],
    agentInactiveGroups: (wsId: string, agentId: string, installationId: string) =>
      ["dingtalk", "groups", wsId, "agent", agentId, "inactive", installationId],
  },
}));

vi.mock("@orvilo/core/api", () => ({
  api: {
    registerDingTalkBYO: mockRegisterBYO,
    deleteDingTalkInstallation: mockDeleteInstallation,
    forgetDingTalkGroup: vi.fn(),
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

vi.mock("sonner", () => ({
  toast: { success: vi.fn(), error: vi.fn(), message: vi.fn() },
}));

vi.mock("../../platform", () => ({ openExternal: mockOpenExternal }));

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

import { toast } from "sonner";
import { renderWithI18n } from "../../test/i18n";
import { DingTalkAgentBindButton, DingTalkTab } from "./dingtalk-tab";

afterEach(cleanup);

/**
 * `lobe: true` mounts the theme bridge on demand, so every assertion after a
 * bridged render must start with an async query — until the bridge's module
 * resolves the tree is a `Suspense` fallback of `null`. It defaults to **off**
 * here because most of this file renders `DingTalkAgentBindButton`, the
 * agent-pane half, which produces no Lobe element at all; the cases that assert
 * emptiness keep the flag off on purpose, so that what makes the container
 * empty is the component and not a pending module load.
 */
function renderUI(children: ReactNode, { lobe = false }: { lobe?: boolean } = {}) {
  return renderWithI18n(<>{children}</>, { lobe });
}

/**
 * Whether the confirm dialog leaves the document within `timeout`. Polls,
 * because a check that samples at a fixed moment cannot tell an open dialog
 * from a closing one, and the text is the dialog's own copy — the imperative
 * confirm is the base-ui `Modal`, not antd's, so there is no
 * `.ant-modal-confirm` to query.
 *
 * The timeout is a parameter because the two callers want opposite things from
 * it: the success path passes a budget (polling returns the instant the node
 * goes, so a generous one is free), the failure path a fixed window (spent in
 * full, and only the one that would catch a mutated build).
 */
async function dialogLeavesWithin(timeout: number): Promise<boolean> {
  try {
    await waitFor(
      () => expect(screen.queryByText("Disconnect this DingTalk bot?")).toBeNull(),
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

describe("DingTalk deployment setup policy", () => {
  beforeEach(resetFixtures);

  it.each(["server_configured", "disabled"] as const)("keeps %s credential lifecycle read-only", async (mode) => {
    configStore.getState().setMessagingConfig({ mode, setupWritable: true, platforms: [] });
    const entry = renderUI(<DingTalkAgentBindButton agentId="agent-1" />);
    expect(screen.queryByTestId("dingtalk-agent-connect")).toBeNull();
    expect(mockRegisterBYO).not.toHaveBeenCalled();
    entry.unmount();

    installationsRef.current = {
      configured: false,
      install_supported: true,
      installations: [{
        id: "existing-installation", agent_id: "agent-1", status: "installed",
        installed_at: "2026-09-10T00:00:00Z", region: "feishu",
        app_id: "app-1", team_id: "team-1", bot_id: "bot-1",
      }],
    };
    // The mixed render needs the bridge: `DingTalkTab` is Lobe, the bind button
    // beside it is not.
    renderUI(<><DingTalkAgentBindButton agentId="agent-1" /><DingTalkTab /></>, { lobe: true });
    expect(await screen.findAllByRole("status", { name: "Connection status" })).toHaveLength(2);
    expect(screen.queryByRole("button", { name: /disconnect/i })).toBeNull();
    expect(mockDeleteInstallation).not.toHaveBeenCalled();
  });
});

function resetFixtures() {
  configStore.getState().setMessagingConfig({ mode: "managed", setupWritable: true, platforms: [] });
  vi.clearAllMocks();
  capturedConfirm.current = null;
  mockConfirmModal.mockImplementation((config: never) => {
    capturedConfirm.current = config as { onOk: () => Promise<unknown> };
    return actualConfirmModal.current?.(config);
  });
  membersRef.current = [{ user_id: "user-1", role: "owner" }];
  agentsRef.current = [
    { id: "agent-1" },
    { id: "agent-7" },
    { id: "agent-8" },
  ];
  installationsRef.current = { installations: [], configured: true, install_supported: true };
  groupsRef.current = {
    data: { groups: [], group_discovery_supported: true },
    isLoading: false,
    isError: false,
    refetch: vi.fn(),
  };
  inactiveGroupsRef.current = {
    data: undefined,
    isLoading: false,
    isError: false,
    refetch: vi.fn(),
    hasNextPage: false,
    isFetchingNextPage: false,
    fetchNextPage: vi.fn(),
  };
}

describe("DingTalkAgentBindButton", () => {
  beforeEach(resetFixtures);

  it("keeps the form open when a malformed response cannot confirm installation", async () => {
    mockRegisterBYO.mockResolvedValue({ id: "", status: "revoked" });
    renderUI(<DingTalkAgentBindButton />);
    await userEvent.click(screen.getByTestId("dingtalk-agent-connect"));
    await userEvent.type(await screen.findByTestId("dingtalk-byo-client-id"), "ding-appkey");
    await userEvent.type(screen.getByTestId("dingtalk-byo-client-secret"), "ding-appsecret");
    await userEvent.click(screen.getByTestId("dingtalk-byo-submit"));
    await waitFor(() => expect(toast.error).toHaveBeenCalledWith(enSettings.dingtalk.byo_failed_toast));
    expect(mockRegisterBYO).toHaveBeenCalledTimes(1);
    expect(toast.success).not.toHaveBeenCalled();
    expect(mockInvalidate).not.toHaveBeenCalled();
    expect(screen.getByTestId("dingtalk-byo-dialog")).toBeInTheDocument();
    expect(screen.getByTestId("dingtalk-byo-client-id")).toHaveValue("ding-appkey");
  });

  it("renders the DingTalk brand mark in the connect button", () => {
    renderUI(<DingTalkAgentBindButton agentId="agent-1" agentName="Bot" />);
    const button = screen.getByTestId("dingtalk-agent-connect");
    expect(button.querySelector('[data-testid="dingtalk-mark"].h-4.w-4')).toBeTruthy();
  });

  it("opens the BYO dialog and submits the pasted AppKey + AppSecret", async () => {
    mockRegisterBYO.mockResolvedValue({ id: "i1", agent_id: "agent-1", status: "installed" });
    renderUI(<DingTalkAgentBindButton agentId="agent-1" agentName="Bot" />);
    await userEvent.click(screen.getByTestId("dingtalk-agent-connect"));
    const idInput = await screen.findByTestId("dingtalk-byo-client-id");
    await userEvent.type(idInput, "ding-appkey");
    await userEvent.type(screen.getByTestId("dingtalk-byo-client-secret"), "ding-appsecret");
    await userEvent.click(screen.getByTestId("dingtalk-byo-submit"));
    await waitFor(() =>
      expect(mockRegisterBYO).toHaveBeenCalledWith("workspace-1", "agent-1", {
        client_id: "ding-appkey",
        client_secret: "ding-appsecret",
      }),
    );
    expect(mockOpenExternal).not.toHaveBeenCalled();
  });

  it("masks both credential inputs as password fields", async () => {
    renderUI(<DingTalkAgentBindButton agentId="agent-1" agentName="Bot" />);
    await userEvent.click(screen.getByTestId("dingtalk-agent-connect"));
    const idInput = await screen.findByTestId("dingtalk-byo-client-id");
    const secretInput = screen.getByTestId("dingtalk-byo-client-secret");
    expect(idInput.getAttribute("type")).toBe("password");
    expect(secretInput.getAttribute("type")).toBe("password");
  });

  it("shows the installation status (not the CTA) when the agent already has an installed bot", () => {
    installationsRef.current = {
      installations: [{ id: "i1", agent_id: "agent-1", status: "installed" }],
      configured: true,
      install_supported: true,
    };
    renderUI(<DingTalkAgentBindButton agentId="agent-1" />);
    expect(screen.getByTestId("dingtalk-agent-bot-installed")).toBeTruthy();
    expect(screen.getByTestId("dingtalk-agent-bot-disconnect")).toBeTruthy();
    expect(screen.queryByTestId("dingtalk-agent-connect")).toBeNull();
  });

  it.each([
    { role: "owner", ownsAgent: false, canManage: true },
    { role: "owner", ownsAgent: true, canManage: true },
    { role: "admin", ownsAgent: false, canManage: true },
    { role: "admin", ownsAgent: true, canManage: true },
    { role: "member", ownsAgent: false, canManage: false },
    { role: "member", ownsAgent: true, canManage: true },
  ] as const)(
    "applies the Agent Detail permission matrix: role=$role ownsAgent=$ownsAgent",
    ({ role, ownsAgent, canManage }) => {
      membersRef.current = [{ user_id: "user-1", role }];
      renderUI(
        <DingTalkAgentBindButton
          agentId="agent-1"
          agentOwnerId={ownsAgent ? "user-1" : "user-2"}
        />,
      );
      expect(screen.queryByTestId("dingtalk-agent-connect") !== null).toBe(canManage);
    },
  );

  it("renders no management entry for a user without workspace membership", () => {
    membersRef.current = [];
    const { container } = renderUI(
      <DingTalkAgentBindButton agentId="agent-1" agentOwnerId="user-1" />,
    );
    expect(container.firstChild).toBeNull();
  });

  it("renders nothing when install is unavailable and the agent is unbound", () => {
    installationsRef.current = { installations: [], configured: true, install_supported: false };
    const { container } = renderUI(<DingTalkAgentBindButton agentId="agent-1" />);
    expect(container.firstChild).toBeNull();
  });
});

describe("DingTalkTab", () => {
  beforeEach(resetFixtures);

  it.each([
    { role: "owner", canManage: true },
    { role: "admin", canManage: true },
    { role: "member", canManage: false },
  ] as const)(
    "applies the Settings permission matrix for role=$role",
    async ({ role, canManage }) => {
      // Settings intentionally has no Agent-owner input: every plain member,
      // including someone who owns one of these Agents, stays read-only here.
      membersRef.current = [{ user_id: "user-1", role }];
      installationsRef.current = {
        installations: [{
          id: "i-role-matrix",
          agent_id: "agent-7",
          status: "installed",
          installed_at: "2026-08-19T00:00:00Z",
          bound_dingtalk_user_ids: ["staff-role-matrix"],
        }],
        configured: true,
        install_supported: true,
      };
      groupsRef.current.data.groups = [{
        conversation_id: "cid-role-matrix",
        conversation_title: "Role matrix group",
        bots: [{
          installation_id: "i-role-matrix",
          agent_id: "agent-7",
          bot_name: "Role matrix bot",
        }],
      }];

      renderUI(<DingTalkTab />, { lobe: true });
      // The section heading is the group's title now, and `Form.Group` renders
      // its label as a bare div — the name lives on the wrapper `SettingsGroup`
      // adds. This is also the async anchor a bridged render needs.
      expect(
        await screen.findByRole("group", { name: "Bot installation records" }),
      ).toBeInTheDocument();
      expect(screen.queryByRole("button", { name: /Disconnect/i }) !== null).toBe(canManage);
      expect(
        screen.queryByRole("button", { name: "Role matrix bot" }) !== null,
      ).toBe(canManage);
      expect(screen.queryByText(/staff-role-matrix/)).toBeNull();
      expect(screen.getByText("Role matrix bot")).toBeTruthy();
      expect(screen.getByText("Role matrix group")).toBeTruthy();
      expect(
        screen.getByText(enSettings.dingtalk.groups_overview_description),
      ).toBeTruthy();
      expect(screen.getByTestId("dingtalk-installation-metadata").textContent).toContain(
        "Installed",
      );
    },
  );

  it("surfaces the not-enabled notice when the deployment has no DingTalk key", async () => {
    installationsRef.current = { installations: [], configured: false, install_supported: false };
    renderUI(<DingTalkTab />, { lobe: true });
    expect(await screen.findByText(/DingTalk integration not enabled/i)).toBeTruthy();
  });

  it("shows the empty state when configured but nothing is connected", async () => {
    renderUI(<DingTalkTab />, { lobe: true });
    expect(await screen.findByText(/No bots installed yet/i)).toBeTruthy();
  });

  it("shows linked Staff IDs from the bot name without adding them to the row", async () => {
    installationsRef.current = {
      installations: [{
        id: "i1",
        agent_id: "agent-7",
        status: "installed",
        bound_dingtalk_user_ids: ["staff-1001", "staff-1002"],
      }],
      configured: true,
      install_supported: true,
    };
    groupsRef.current.data.groups = [{
      conversation_id: "cid-linked",
      conversation_title: "Linked group",
      bots: [{
        installation_id: "i1",
        agent_id: "agent-7",
        bot_name: "Linked Bot",
      }],
    }];
    renderUI(<DingTalkTab />, { lobe: true });
    expect(await screen.findByText("Agent agent-7")).toBeTruthy();
    const botName = screen.getByRole("button", { name: "Linked Bot" });
    expect(screen.queryByText(/Linked staff ID:/i)).toBeNull();
    await userEvent.hover(botName);
    expect(
      await screen.findByText("Linked staff ID: staff-1001, staff-1002"),
    ).toBeTruthy();
    expect(screen.getByText(/Disconnect/i)).toBeTruthy();
  });

  it("does not render a linked identity when this member has no DingTalk binding", async () => {
    installationsRef.current = {
      installations: [{ id: "i1", agent_id: "agent-7", status: "installed" }],
      configured: true,
      install_supported: true,
    };
    renderUI(<DingTalkTab />, { lobe: true });
    // Positive anchor first: under the bridge an un-anchored negative is
    // satisfied by the `Suspense` fallback rather than by the component.
    expect(await screen.findByText("Agent agent-7")).toBeTruthy();
    expect(screen.queryByText(/Linked staff ID:/i)).toBeNull();
  });

  it("hides linked DingTalk identities from a regular workspace member", async () => {
    membersRef.current = [{ user_id: "user-1", role: "member" }];
    installationsRef.current = {
      installations: [{
        id: "i1",
        agent_id: "agent-7",
        status: "installed",
        bound_dingtalk_user_ids: ["staff-must-stay-private"],
      }],
      configured: true,
      install_supported: true,
    };
    renderUI(<DingTalkTab />, { lobe: true });
    expect(await screen.findByText("Agent agent-7")).toBeTruthy();
    expect(screen.queryByText(/staff-must-stay-private/)).toBeNull();
    expect(screen.queryByText(/Linked staff ID:/i)).toBeNull();
    expect(screen.getByRole("group", { name: "Bot installation records" })).toBeTruthy();
    const overviewDescription = screen.getByText(
      enSettings.dingtalk.groups_overview_description,
    );
    expect(overviewDescription.classList).toContain("text-caption");
  });

  it("shows every observed Agent → Bot → Group relationship to an admin", async () => {
    membersRef.current = [{ user_id: "user-1", role: "admin" }];
    installationsRef.current = {
      installations: [
        {
          id: "i1",
          agent_id: "agent-7",
          status: "installed",
          installed_at: "2026-08-19T00:00:00Z",
          bound_dingtalk_user_ids: ["staff-1001"],
        },
      ],
      configured: true,
      install_supported: true,
    };
    groupsRef.current.data.groups = [
      {
        conversation_id: "cid-platform",
        conversation_title: "Platform team",
        bots: [
          {
            installation_id: "i1",
            agent_id: "agent-7",
            bot_name: "Release Bot",
            bot_identity_issue: "missing_qyapi_chat_manage",
          },
        ],
      },
    ];

    renderUI(<DingTalkTab />, { lobe: true });
    expect(await screen.findByText("Agent agent-7")).toBeTruthy();
    expect(screen.getByRole("group", { name: "Bot installation records" })).toBeTruthy();
    expect(screen.getByText("Release Bot")).toBeTruthy();
    expect(
      screen.getByRole("button", { name: /qyapi_chat_manage/i }),
    ).toBeTruthy();
    const connectedLabel = screen.getByRole("status", { name: "Connection status" });
    const connectedStatus = connectedLabel.parentElement?.parentElement;
    expect(connectedStatus?.classList).toContain(
      "text-caption",
    );
    expect(connectedStatus?.classList).not.toContain(
      "text-micro",
    );
    const metadata = screen.getByTestId("dingtalk-installation-metadata");
    expect(metadata.contains(connectedLabel)).toBe(false);
    expect(metadata.parentElement).toBe(connectedStatus?.parentElement);
    expect(metadata.textContent).toContain("Installed");
    expect(metadata.parentElement?.textContent).toContain(
      "Status unavailableRelease Bot·Installed",
    );
    expect(metadata.parentElement?.textContent).not.toContain("Linked staff ID");
    expect(metadata.textContent).not.toContain("Release Bot");
    expect(metadata.textContent).not.toMatch(/\d{1,2}:\d{2}:\d{2}/);
    expect(screen.queryByText(/Connected to Agent/i)).toBeNull();
    const groupTitle = screen.getByText("Platform team");
    const forgetButton = screen.getByRole("button", { name: "Forget" });
    expect(groupTitle.parentElement?.contains(forgetButton)).toBe(true);
    expect(forgetButton.classList).toContain("opacity-0");
    expect(forgetButton.classList).toContain("group-hover:opacity-100");
    expect(forgetButton.classList).toContain("group-focus-within:opacity-100");
    expect(
      screen.getByRole("heading", { name: "Recent groups", level: 4 }),
    ).toBeTruthy();
    const overviewDescriptions = screen.getAllByText(
      enSettings.dingtalk.groups_overview_description,
    );
    expect(overviewDescriptions).toHaveLength(1);
    expect(overviewDescriptions[0]?.classList).toContain("text-caption");
    expect(overviewDescriptions[0]?.classList).not.toContain("text-micro");
    expect(screen.getByTestId("dingtalk-bot-groups").parentElement).toBe(
      screen.getByTestId("dingtalk-installation-row"),
    );
    expect(screen.getByTestId("dingtalk-installation-row").classList).toContain("py-6");
    // The row's other half of the same claim: it sits inside the group that
    // replaced the card. `[data-slot="card"]` is gone with shadcn's `Card`, so
    // the host is now named by the group's accessible name rather than by a
    // base-ui slot attribute.
    expect(
      screen.getByRole("group", { name: "Bot installation records" }).contains(
        screen.getByTestId("dingtalk-installation-row"),
      ),
    ).toBe(true);
    const conversationId = screen.getByLabelText(
      "DingTalk group conversation ID cid-platform",
    );
    expect(conversationId.textContent).toBe("cid-platform");
    expect(conversationId.classList).toContain("text-faint-foreground");
    expect(conversationId.classList).toContain("group-hover:text-muted-foreground");
    await userEvent.hover(conversationId);
    await waitFor(() =>
      expect(document.querySelector('[data-slot="tooltip-content"]')).toBeTruthy(),
    );
    const tooltip = document.querySelector('[data-slot="tooltip-content"]');
    expect(tooltip?.textContent).toBe("DingTalk group conversation ID");
    expect(tooltip?.textContent).not.toContain("cid-platform");
    expect(screen.getAllByText("cid-platform")).toHaveLength(1);
  });

  it("sorts active groups by recency, then inactive groups by title and conversation ID", async () => {
    installationsRef.current = {
      installations: [{ id: "i1", agent_id: "agent-7", status: "installed" }],
      configured: true,
      install_supported: true,
    };
    groupsRef.current.data.groups = [
      {
        conversation_id: "cid-alpha-b",
        conversation_title: "Alpha",
        bots: [{ installation_id: "i1", agent_id: "agent-7" }],
      },
      {
        conversation_id: "cid-old",
        conversation_title: "Zulu",
        bots: [{
          installation_id: "i1",
          agent_id: "agent-7",
          last_active_at: "2026-08-19T08:00:00Z",
        }],
      },
      {
        conversation_id: "cid-untitled",
        conversation_title: "",
        bots: [{ installation_id: "i1", agent_id: "agent-7" }],
      },
      {
        conversation_id: "cid-recent",
        conversation_title: "Beta",
        bots: [{
          installation_id: "i1",
          agent_id: "agent-7",
          last_active_at: "2026-08-19T09:00:00Z",
        }],
      },
      {
        conversation_id: "cid-alpha-a",
        conversation_title: "Alpha",
        bots: [{ installation_id: "i1", agent_id: "agent-7" }],
      },
    ];

    renderUI(<DingTalkTab />, { lobe: true });
    await screen.findByTestId("dingtalk-bot-groups");
    expect(
      screen.getAllByTestId("dingtalk-group-item").map((item) =>
        item.querySelector("code")?.textContent,
      ),
    ).toEqual([
      "cid-recent",
      "cid-old",
      "cid-alpha-a",
      "cid-alpha-b",
      "cid-untitled",
    ]);
  });

  it("globally sorts inactive groups after merging loaded pages", async () => {
    installationsRef.current = {
      installations: [{ id: "i1", agent_id: "agent-7", status: "installed" }],
      configured: true,
      install_supported: true,
    };
    groupsRef.current.data = {
      groups: [],
      group_discovery_supported: true,
      inactive_group_counts: { i1: 4 },
    };
    inactiveGroupsRef.current.data = {
      pages: [
        {
          groups: [
            {
              conversation_id: "cid-zulu",
              conversation_title: "Zulu",
              bots: [{ installation_id: "i1", agent_id: "agent-7" }],
            },
            {
              conversation_id: "cid-bravo-b",
              conversation_title: "Bravo",
              bots: [{ installation_id: "i1", agent_id: "agent-7" }],
            },
          ],
          next_offset: 2,
        },
        {
          groups: [
            {
              conversation_id: "cid-alpha",
              conversation_title: "Alpha",
              bots: [{ installation_id: "i1", agent_id: "agent-7" }],
            },
            {
              conversation_id: "cid-bravo-a",
              conversation_title: "Bravo",
              bots: [{ installation_id: "i1", agent_id: "agent-7" }],
            },
          ],
          next_offset: null,
        },
      ],
    };

    renderUI(<DingTalkTab />, { lobe: true });
    await userEvent.click(await screen.findByRole("button", { name: /long inactive/i }));
    expect(
      screen.getAllByTestId("dingtalk-group-item").map((item) =>
        item.querySelector("code")?.textContent,
      ),
    ).toEqual(["cid-alpha", "cid-bravo-a", "cid-bravo-b", "cid-zulu"]);
  });

  it("renders discovery UI only when the backend explicitly supports it", async () => {
    installationsRef.current = {
      installations: [{ id: "i1", agent_id: "agent-7", status: "installed" }],
      configured: true,
      install_supported: true,
    };
    groupsRef.current.data = {
      groups: [],
      group_discovery_supported: false,
    };

    renderUI(<DingTalkTab />, { lobe: true });
    expect(await screen.findByRole("status", { name: "Connection status" })).toBeTruthy();
    expect(screen.queryByText("Identity unavailable")).toBeNull();
    expect(screen.queryByTestId("dingtalk-bot-groups")).toBeNull();
    expect(
      screen.queryByText(enSettings.dingtalk.groups_overview_description),
    ).toBeNull();
  });

  it("keeps loading and retryable error states visible before capability data arrives", async () => {
    installationsRef.current = {
      installations: [{ id: "i1", agent_id: "agent-7", status: "installed" }],
      configured: true,
      install_supported: true,
    };
    groupsRef.current.isLoading = true;
    groupsRef.current.data = undefined as never;

    const loading = renderUI(<DingTalkTab />, { lobe: true });
    expect(await screen.findByText("Loading groups…")).toBeTruthy();
    loading.unmount();

    groupsRef.current.isLoading = false;
    groupsRef.current.isError = true;
    groupsRef.current.data = undefined as never;
    renderUI(<DingTalkTab />, { lobe: true });

    await userEvent.click(await screen.findByRole("button", { name: "Retry" }));
    expect(groupsRef.current.refetch).toHaveBeenCalledOnce();
  });

  it("shows only the server-filtered Agent, bot, groups, and install time to a regular member", async () => {
    membersRef.current = [{ user_id: "user-1", role: "member" }];
    installationsRef.current = {
      installations: [
        { id: "i1", agent_id: "agent-7", status: "installed", installed_at: "" },
      ],
      configured: true,
      install_supported: true,
    };
    groupsRef.current.data.groups = [
      {
        conversation_id: "cid-private",
        conversation_title: "Visible group",
        bots: [
          {
            installation_id: "i1",
            agent_id: "agent-7",
            bot_name: "Visible Bot",
            bot_identity_issue: "missing_qyapi_chat_manage",
          },
        ],
      },
    ];

    renderUI(<DingTalkTab />, { lobe: true });
    expect(await screen.findByText("Agent agent-7")).toBeTruthy();
    expect(screen.getByText("Visible Bot")).toBeTruthy();
    expect(screen.getByText("Visible group")).toBeTruthy();
    expect(screen.getByTestId("dingtalk-bot-groups")).toBeTruthy();
    expect(screen.getByTestId("dingtalk-installation-metadata").textContent).toContain(
      "Installed —",
    );
    expect(screen.queryByRole("button", { name: /Disconnect/i })).toBeNull();
    expect(screen.queryByRole("button", { name: /qyapi_chat_manage/i })).toBeNull();
  });

  it("shows an unavailable bot identity without admin remediation to a regular member", async () => {
    membersRef.current = [{ user_id: "user-1", role: "member" }];
    installationsRef.current = {
      installations: [
        { id: "i1", agent_id: "agent-7", status: "installed", installed_at: "" },
      ],
      configured: true,
      install_supported: true,
    };
    groupsRef.current.data.groups = [
      {
        conversation_id: "cid-visible",
        conversation_title: "Visible group",
        bots: [
          {
            installation_id: "i1",
            agent_id: "agent-7",
            bot_name: "",
            bot_identity_issue: "missing_qyapi_chat_manage",
          },
        ],
      },
    ];

    renderUI(<DingTalkTab />, { lobe: true });
    expect(await screen.findByText("Identity unavailable")).toBeTruthy();
    expect(screen.queryByRole("button", { name: /qyapi_chat_manage/i })).toBeNull();
  });

  it("filters a legacy server row when the member cannot see its Agent", async () => {
    membersRef.current = [{ user_id: "user-1", role: "member" }];
    agentsRef.current = [{ id: "agent-7" }];
    installationsRef.current = {
      installations: [
        {
          id: "i-private",
          agent_id: "agent-private",
          status: "installed",
          installed_at: "2026-08-19T00:00:00Z",
        },
      ],
      configured: true,
      install_supported: true,
    };

    renderUI(<DingTalkTab />, { lobe: true });
    // Positive anchor first — the two negatives below are only meaningful once
    // the empty state has actually rendered.
    expect(await screen.findByText("No bots installed yet")).toBeTruthy();
    expect(screen.queryByText("Agent agent-private")).toBeNull();
    expect(screen.queryByTestId("dingtalk-installation-row")).toBeNull();
  });

  it("labels an orphaned admin-only installation without linking to a missing Agent", async () => {
    installationsRef.current = {
      installations: [
        {
          id: "i-orphan",
          agent_id: "agent-deleted",
          agent_available: false,
          status: "installed",
          installed_at: "2026-08-19T00:00:00Z",
        },
      ],
      configured: true,
      install_supported: true,
    };

    renderUI(<DingTalkTab />, { lobe: true });
    expect(await screen.findByText("Deleted Agent")).toBeTruthy();
    expect(screen.getByTestId("actor-avatar").getAttribute("data-profile-link")).toBe(
      "false",
    );
    expect(screen.getByRole("button", { name: /Disconnect/i })).toBeTruthy();
  });

  it("shows a placeholder instead of 'Invalid Date' when installed_at is missing or malformed", async () => {
    installationsRef.current = {
      installations: [
        { id: "i1", agent_id: "agent-7", status: "installed", installed_at: "" },
        { id: "i2", agent_id: "agent-8", status: "installed", installed_at: "not-a-date" },
      ],
      configured: true,
      install_supported: true,
    };
    renderUI(<DingTalkTab />, { lobe: true });
    // Positive anchor first: both rows must be on screen before "no Invalid
    // Date anywhere" says anything.
    expect(await screen.findAllByTestId("dingtalk-installation-row")).toHaveLength(2);
    expect(screen.queryByText(/Invalid Date/i)).toBeNull();
  });

  /**
   * The caller's half of the `confirmModal` contract.
   *
   * `confirmModal` closes on the line after `onOk` unless `onOk` returns a
   * promise, and stays open when that promise rejects — so a disconnect that
   * fails must leave the confirmation up rather than dismiss as though the bot
   * were gone. The first case shows the success path *does* close, which is
   * what gives the second one's negative its meaning; the third reads the
   * contract off the config the wrapper handed the library, where it is not a
   * DOM question at all.
   */
  it("disconnects only after the confirmation, and the dialog closes when it succeeds", async () => {
    mockDeleteInstallation.mockResolvedValue(undefined);
    installationsRef.current = {
      installations: [{ id: "i1", agent_id: "agent-7", status: "installed" }],
      configured: true,
      install_supported: true,
    };
    renderUI(<DingTalkTab />, { lobe: true });
    await userEvent.click(await screen.findByRole("button", { name: "Disconnect" }));
    expect(mockDeleteInstallation).not.toHaveBeenCalled();

    await screen.findByText("Disconnect this DingTalk bot?");
    // Two buttons carry this name now — the row's and the dialog's — and the
    // dialog's is last in document order (it is portalled to the end of
    // `<body>`), so `.at(-1)` is the one that runs the request.
    const confirmButtons = await screen.findAllByRole("button", { name: /^Disconnect$/ });
    await userEvent.click(confirmButtons.at(-1)!);
    await waitFor(() => {
      expect(mockDeleteInstallation).toHaveBeenCalledWith("workspace-1", "i1");
    });
    expect(mockInvalidate).toHaveBeenCalled();
    expect(await dialogLeavesWithin(CLOSE_BUDGET_MS)).toBe(true);
  });

  it("keeps the confirmation up when the disconnect request fails", async () => {
    mockDeleteInstallation.mockRejectedValue(new Error("network failed"));
    installationsRef.current = {
      installations: [{ id: "i1", agent_id: "agent-7", status: "installed" }],
      configured: true,
      install_supported: true,
    };
    renderUI(<DingTalkTab />, { lobe: true });
    await userEvent.click(await screen.findByRole("button", { name: "Disconnect" }));
    await screen.findByText("Disconnect this DingTalk bot?");
    const confirmButtons = await screen.findAllByRole("button", { name: /^Disconnect$/ });
    await userEvent.click(confirmButtons.at(-1)!);

    await waitFor(() => expect(toast.error).toHaveBeenCalledWith("network failed"));
    expect(await dialogLeavesWithin(CLOSE_WINDOW_MS)).toBe(false);

    // It is deliberately still open, and the confirm stack is module state that
    // outlives this case — an open dialog would land on the next one in this
    // file and read exactly like a flake. Close it and wait for it to go.
    await userEvent.click(screen.getByRole("button", { name: "Cancel" }));
    expect(await dialogLeavesWithin(CLOSE_BUDGET_MS)).toBe(true);
  });

  it("hands confirmModal an onOk that rejects when the disconnect fails", async () => {
    mockDeleteInstallation.mockRejectedValue(new Error("network failed"));
    installationsRef.current = {
      installations: [{ id: "i1", agent_id: "agent-7", status: "installed" }],
      configured: true,
      install_supported: true,
    };
    renderUI(<DingTalkTab />, { lobe: true });
    await userEvent.click(await screen.findByRole("button", { name: "Disconnect" }));
    expect(capturedConfirm.current).not.toBeNull();

    await expect(capturedConfirm.current?.onOk()).rejects.toThrow("network failed");
    expect(toast.error).toHaveBeenCalledWith("network failed");
  });
});
