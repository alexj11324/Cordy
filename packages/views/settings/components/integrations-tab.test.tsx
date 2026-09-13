// @vitest-environment jsdom

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import "@testing-library/jest-dom/vitest";
import { cleanup, fireEvent, screen, within } from "@testing-library/react";
import { renderWithI18n } from "../../test/i18n";
import { ApiError } from "@orvilo/core/api";
import { configStore } from "@orvilo/core/config";
import {
  BILLING_WORKSPACE_SUBSCRIPTIONS_FLAG,
  COMPOSIO_MCP_APPS_FLAG,
  LINEAR_INSTALLATION_FOUNDATION_FLAG,
} from "@orvilo/core/feature-flags";

const composioErrorRef = vi.hoisted(() => ({
  current: null as Error | null,
}));
const queryCallsRef = vi.hoisted(() => ({
  current: [] as { queryKey: unknown[]; enabled?: boolean }[],
}));
const authUserRef = vi.hoisted(() => ({
  current: null as { id: string; is_guest?: boolean } | null,
}));
const membersRef = vi.hoisted(() => ({
  current: [] as { user_id: string; role: string }[],
}));
const messagingQuotaRef = vi.hoisted(() => ({
  current: undefined as
    | { mode: string; used: number | null; reserved: number | null; limit: number | null }
    | undefined,
}));
const subscriptionSummaryRef = vi.hoisted(() => ({
  current: undefined as
    | {
        entitlement: {
          hostedWorkspaceLimit?: number | null;
          imInstallationLimit?: number | null;
        };
      }
    | undefined,
}));
const channelInstallationsRef = vi.hoisted(() => ({
  current: {} as Partial<Record<
    "lark" | "slack" | "dingtalk" | "wecom" | "telegram" | "weixin",
    {
      configured: boolean;
      install_supported: boolean;
      managed_supported?: boolean;
      installations: {
        id: string;
        agent_id: string | null;
        status: string;
        runtime?: { state: string; observedAt: string | null; errorCode: string | null };
      }[];
    }
  >>,
}));

vi.mock("@tanstack/react-query", () => ({
  useQuery: (opts: { queryKey: unknown[]; enabled?: boolean }) => {
    queryCallsRef.current.push(opts);
    const isMemberQuery = opts.queryKey[opts.queryKey.length - 1] === "members";
    const channel = opts.queryKey[0];
    const isMessagingQuotaQuery = channel === "messaging-quota";
    const isSubscriptionSummaryQuery = channel === "workspace-subscriptions";
    const isChannelInstallationsQuery =
      typeof channel === "string" &&
      channel in channelInstallationsRef.current &&
      opts.queryKey[opts.queryKey.length - 1] === "installations";
    return {
      data: isMemberQuery
        ? membersRef.current
        : isMessagingQuotaQuery
          ? messagingQuotaRef.current
        : isSubscriptionSummaryQuery
          ? subscriptionSummaryRef.current
        : isChannelInstallationsQuery
          ? channelInstallationsRef.current[
              channel as keyof typeof channelInstallationsRef.current
            ]
          : undefined,
      error: opts.enabled === false ? null : composioErrorRef.current,
      isError: opts.enabled !== false && composioErrorRef.current != null,
      isLoading: false,
    };
  },
  queryOptions: <T,>(opts: T) => opts,
  useQueryClient: () => ({ invalidateQueries: vi.fn() }),
}));

vi.mock("@orvilo/core/composio", () => ({
  composioToolkitsOptions: () => ({ queryKey: ["composio", "toolkits"] }),
}));

vi.mock("@orvilo/core/paths", () => ({
  useCurrentWorkspace: () => ({ id: "workspace-1", name: "Acme", slug: "acme" }),
}));

vi.mock("@orvilo/core/auth", () => ({
  useAuthStore: (selector: (state: { user: typeof authUserRef.current }) => unknown) =>
    selector({ user: authUserRef.current }),
}));

for (const channel of ["lark", "slack", "dingtalk", "wecom", "telegram", "weixin"]) {
  vi.doMock(`@orvilo/core/${channel}`, () => ({
    [`${channel}InstallationsOptions`]: (workspaceId: string) => ({
      queryKey: [channel, workspaceId, "installations"],
    }),
    [`${channel}Keys`]: {
      installations: (workspaceId: string) => [channel, workspaceId, "installations"],
    },
  }));
}

vi.mock("./lark-tab", () => ({
  LarkTab: () => <div data-testid="lark-tab" />,
  LarkAgentBindButton: () => <button data-testid="lark-hub-install">Install</button>,
}));

vi.mock("./composio-tab", () => ({
  ComposioTab: () => <div data-testid="composio-tab" />,
}));

vi.mock("./slack-tab", () => ({
  SlackTab: () => <div data-testid="slack-tab" />,
  SlackAgentBindButton: () => <button data-testid="slack-hub-install">Install</button>,
}));

vi.mock("./dingtalk-tab", () => ({
  DingTalkTab: () => <div data-testid="dingtalk-tab" />,
  DingTalkAgentBindButton: () => <button data-testid="dingtalk-hub-install">Install</button>,
}));

vi.mock("./vcs-tab", () => ({
  VCSTab: () => <div data-testid="vcs-tab" />,
}));

vi.mock("./wecom-tab", () => ({
  WecomTab: () => <div data-testid="wecom-tab" />,
  WecomAgentBindButton: () => <button data-testid="wecom-hub-install">Install</button>,
}));

vi.mock("./telegram-tab", () => ({
  TelegramTab: () => <div data-testid="telegram-tab" />,
  TelegramAgentBindButton: () => <button data-testid="telegram-hub-install">Install</button>,
}));

vi.mock("./weixin-tab", () => ({
  WeixinTab: () => <div data-testid="weixin-tab" />,
  WeixinAgentBindButton: () => <button data-testid="weixin-hub-install">Install</button>,
}));

vi.mock("./linear-tab", () => ({
  LinearIntegrationCard: () => <div data-testid="integration-channel-card-linear" />,
}));

import { IntegrationsTab } from "./integrations-tab";

afterEach(cleanup);

// `{ lobe: true }` is required, not decoration. This suite used a bare `render`
// with its own `I18nProvider` — it did not merely omit the option, it bypassed
// the bridge-aware helper entirely — while rendering `IntegrationSetupGuide`
// for real at four call sites and `IntegrationRowMenu` for the channel actions.
// Both components are Lobe now, and Lobe's `Button` throws
// `Please wrap your app with <ConfigProvider> (or <MotionProvider>)` from
// `useMotionComponent` without the bridge. A test file is a host surface.
// Async because the bridge is lazy: until its module resolves the tree is a
// `Suspense` fallback of `null`, so a synchronous first query sees an empty
// `<body>`. The channel card is rendered unconditionally, which makes it the
// one handle every case in this file can wait on.
async function renderTab() {
  const result = renderWithI18n(<IntegrationsTab />, { lobe: true });
  await screen.findByTestId("integration-channel-card-lark");
  return result;
}

function openChannelAction(card: HTMLElement, label: string) {
  fireEvent.click(within(card).getByRole("button", { name: label }));
  fireEvent.click(screen.getByRole("menuitem", { name: label }));
}

describe("Settings IntegrationsTab", () => {
  beforeEach(() => {
    queryCallsRef.current = [];
    composioErrorRef.current = null;
    authUserRef.current = null;
    membersRef.current = [];
    messagingQuotaRef.current = undefined;
    subscriptionSummaryRef.current = undefined;
    channelInstallationsRef.current = {
      lark: { configured: false, install_supported: false, installations: [] },
      slack: { configured: false, install_supported: false, installations: [] },
      dingtalk: { configured: false, install_supported: false, installations: [] },
      wecom: { configured: false, install_supported: false, installations: [] },
      telegram: { configured: false, install_supported: false, installations: [] },
      weixin: { configured: false, install_supported: false, installations: [] },
    };
    configStore.getState().setFeatureFlags({
      [COMPOSIO_MCP_APPS_FLAG]: true,
      [LINEAR_INSTALLATION_FOUNDATION_FLAG]: false,
    });
    // Reset the self-host-only VCS gate to its default (hidden) so tests stay
    // isolated; individual tests opt in below.
    configStore.getState().setAuthConfig({ allowSignup: true, vcsIntegrationAvailable: false });
    configStore.getState().setMessagingConfig({
      mode: "managed",
      setupWritable: true,
      platforms: [],
    });
  });

  it("renders messaging integrations as workspace cards instead of expanded forms", async () => {
    await renderTab();

    for (const channel of ["lark", "slack", "dingtalk", "wecom", "telegram", "weixin"]) {
      expect(screen.getByTestId(`integration-channel-card-${channel}`)).toBeInTheDocument();
      expect(screen.queryByTestId(`${channel}-tab`)).toBeNull();
    }
  });

  it("shows used plus reserved hosted Agent turns", async () => {
    messagingQuotaRef.current = { mode: "managed", used: 7, reserved: 2, limit: 100 };
    await renderTab();
    expect(screen.getByTestId("messaging-quota")).toHaveTextContent(
      "9 of 100 Agent turns used this period",
    );
  });

  it("treats disabled hosted-turn policy as unlimited", async () => {
    messagingQuotaRef.current = { mode: "disabled", used: null, reserved: null, limit: null };
    await renderTab();
    expect(screen.getByTestId("messaging-quota")).toHaveTextContent("Unlimited");
  });

  it("shows hosted installation and owned-workspace capacity", async () => {
    configStore.getState().setFeatureFlags({
      [BILLING_WORKSPACE_SUBSCRIPTIONS_FLAG]: true,
      [COMPOSIO_MCP_APPS_FLAG]: true,
      [LINEAR_INSTALLATION_FOUNDATION_FLAG]: false,
    });
    subscriptionSummaryRef.current = {
      entitlement: {
        hostedWorkspaceLimit: 4,
        imInstallationLimit: 3,
      },
    };
    channelInstallationsRef.current.slack = {
      configured: true,
      install_supported: true,
      installations: [
        { id: "hub-1", agent_id: null, status: "installed" },
      ],
    };

    await renderTab();

    expect(screen.getByTestId("messaging-quota")).toHaveTextContent(
      "Hosted installations: 1 of 3 installed",
    );
    expect(screen.getByTestId("messaging-quota")).toHaveTextContent(
      "Owned hosted workspaces: up to 4",
    );
  });

  it("shows a runtime-confirmed workspace installation as connected", async () => {
    authUserRef.current = { id: "admin-user" };
    membersRef.current = [{ user_id: "admin-user", role: "owner" }];
    channelInstallationsRef.current.slack = {
      configured: true,
      install_supported: true,
      installations: [{
        id: "hub-1",
        agent_id: "",
        status: "installed",
        runtime: {
          state: "healthy",
          observedAt: "2026-09-03T12:00:00Z",
          errorCode: null,
        },
      }],
    };

    await renderTab();

    const card = screen.getByTestId("integration-channel-card-slack");
    expect(within(card).getByText("Connected")).toBeInTheDocument();
    expect(within(card).getByRole("button", { name: "Manage" })).toBeInTheDocument();
  });

  it("opens server setup guidance without exposing credential actions", async () => {
    authUserRef.current = { id: "admin-user" };
    membersRef.current = [{ user_id: "admin-user", role: "owner" }];
    configStore.getState().setMessagingConfig({
      mode: "server_configured",
      setupWritable: false,
      platforms: [],
    });

    await renderTab();

    expect(screen.getAllByRole("button", { name: "View setup" })).toHaveLength(6);
    openChannelAction(screen.getByTestId("integration-channel-card-dingtalk"), "View setup");
    expect(screen.getByRole("button", { name: "Server setup instructions" })).toBeInTheDocument();
    expect(screen.queryByTestId("dingtalk-hub-install")).toBeNull();
    expect(screen.queryByTestId("integration-setup-guide-dingtalk")).toBeNull();
  });

  it("keeps existing self-hosted connections readable for workspace members", async () => {
    authUserRef.current = { id: "member-user" };
    membersRef.current = [{ user_id: "member-user", role: "member" }];
    configStore.getState().setMessagingConfig({ mode: "server_configured", setupWritable: false, platforms: [] });
    channelInstallationsRef.current.slack = {
      configured: true, install_supported: true,
      installations: [{ id: "existing-slack", agent_id: null, status: "installed" }],
    };
    await renderTab();
    openChannelAction(screen.getByTestId("integration-channel-card-slack"), "View details");
    expect(screen.getByTestId("slack-tab")).toBeInTheDocument();
    expect(screen.queryByTestId("slack-hub-install")).toBeNull();
  });

  it("offers Slack token setup when hosted OAuth is unavailable", async () => {
    authUserRef.current = { id: "admin-user" };
    membersRef.current = [{ user_id: "admin-user", role: "owner" }];
    channelInstallationsRef.current.slack = {
      configured: true, install_supported: true, managed_supported: false, installations: [],
    };
    await renderTab();
    openChannelAction(screen.getByTestId("integration-channel-card-slack"), "Configure");
    expect(screen.getByTestId("slack-hub-install")).toBeInTheDocument();
    expect(screen.queryByTestId("slack-tab")).toBeNull();
    expect(screen.getByTestId("integration-setup-guide-slack")).toHaveTextContent("xoxb-");
  });

  it("uses Slack OAuth only when the server advertises managed support", async () => {
    authUserRef.current = { id: "admin-user" };
    membersRef.current = [{ user_id: "admin-user", role: "owner" }];
    channelInstallationsRef.current.slack = {
      configured: true, install_supported: true, managed_supported: true, installations: [],
    };
    await renderTab();
    openChannelAction(screen.getByTestId("integration-channel-card-slack"), "Configure");
    expect(screen.getByTestId("slack-tab")).toBeInTheDocument();
    expect(screen.queryByTestId("slack-hub-install")).toBeNull();
    expect(screen.getByTestId("integration-setup-guide-slack")).not.toHaveTextContent("xoxb-");
  });

  it("keeps workspace setup reachable beside an existing Agent connection", async () => {
    authUserRef.current = { id: "admin-user" };
    membersRef.current = [{ user_id: "admin-user", role: "owner" }];
    channelInstallationsRef.current.telegram = {
      configured: true, install_supported: true,
      installations: [{ id: "agent-telegram", agent_id: "agent-1", status: "installed" }],
    };
    await renderTab();
    openChannelAction(screen.getByTestId("integration-channel-card-telegram"), "Manage");
    expect(screen.getByTestId("telegram-tab")).toBeInTheDocument();
    expect(screen.getByTestId("telegram-hub-install")).toBeInTheDocument();
  });

  it("opens the platform setup guide without exposing deployment variables", async () => {
    authUserRef.current = { id: "admin-user" };
    membersRef.current = [{ user_id: "admin-user", role: "owner" }];
    channelInstallationsRef.current.dingtalk = {
      configured: true, install_supported: true, installations: [],
    };

    await renderTab();
    openChannelAction(screen.getByTestId("integration-channel-card-dingtalk"), "Configure");

    expect(screen.getByTestId("integration-setup-guide-dingtalk")).toBeInTheDocument();
    expect(screen.getByTestId("dingtalk-hub-install")).toBeInTheDocument();
    expect(screen.queryByText("ORVILO_DINGTALK_SECRET_KEY")).toBeNull();
  });

  it("hides Composio and disables the toolkits query when the feature flag is off", async () => {    configStore.getState().setFeatureFlags({ [COMPOSIO_MCP_APPS_FLAG]: false });

    await renderTab();

    expect(screen.queryByTestId("composio-tab")).toBeNull();
    const composioQuery = queryCallsRef.current.find(
      (query) => query.queryKey[0] === "composio",
    );
    expect(composioQuery?.enabled).toBe(false);
  });

  it("shows Composio when the feature flag is on and the integration is configured", async () => {
    await renderTab();

    expect(screen.getByTestId("composio-tab")).toBeInTheDocument();
    const composioQuery = queryCallsRef.current.find(
      (query) => query.queryKey[0] === "composio",
    );
    expect(composioQuery?.enabled).toBe(true);
  });

  it("shows Linear by default when the server has not supplied a release flag", async () => {
    configStore.getState().setFeatureFlags({});
    await renderTab();
    expect(screen.getByTestId("integration-channel-card-linear")).toBeInTheDocument();
  });

  it("shows Linear only when its installation feature is enabled", async () => {
    await renderTab();
    expect(screen.queryByTestId("integration-channel-card-linear")).toBeNull();

    cleanup();
    configStore.getState().setFeatureFlags({
      [COMPOSIO_MCP_APPS_FLAG]: true,
      [LINEAR_INSTALLATION_FOUNDATION_FLAG]: true,
    });
    await renderTab();
    expect(screen.getByTestId("integration-channel-card-linear")).toBeInTheDocument();
  });

  it("shows each channel description beside its title", async () => {
    await renderTab();

    for (const channel of ["lark", "slack", "dingtalk", "wecom", "weixin", "telegram"]) {
      const card = screen.getByTestId(`integration-channel-card-${channel}`);
      const icon = screen.getByTestId(`integration-channel-icon-${channel}`);
      expect(card.querySelector("[data-slot=item-title]")).not.toBeNull();
      expect(card.querySelector("[data-slot=item-description]")).not.toBeNull();
      expect(icon).not.toHaveClass("border");
      expect(icon).not.toHaveClass("bg-muted/40");
    }
  });

  // Reaching for a generic lucide glyph is how Slack and WeCom ended up sharing
  // one speech bubble, with nothing on the row saying which platform it was
  // (#6585). Requiring five distinct shapes is the cheap guard against a
  // regression to that.
  it("gives every channel its own brand mark", async () => {
    await renderTab();

    const shapes = ["lark", "slack", "dingtalk", "wecom", "weixin", "telegram"].map(
      (channel) => screen.getByTestId(`integration-channel-icon-${channel}`).innerHTML,
    );

    expect(new Set(shapes).size).toBe(shapes.length);
  });

  it("hides Composio when the feature flag is on but the server reports 503", async () => {
    composioErrorRef.current = new ApiError("unavailable", 503, "Service Unavailable");

    await renderTab();

    expect(screen.queryByTestId("composio-tab")).toBeNull();
  });

  it("hides the Git providers section when the deployment reports it unavailable", async () => {
    // Default (managed cloud / older server): vcsIntegrationAvailable is false.
    await renderTab();

    expect(screen.queryByTestId("vcs-tab")).toBeNull();
  });

  it("shows the Git providers section on a self-hosted deployment that enables it", async () => {
    configStore.getState().setAuthConfig({ allowSignup: true, vcsIntegrationAvailable: true });

    await renderTab();

    expect(screen.getByTestId("vcs-tab")).toBeInTheDocument();
  });

  it("renders the centered page chrome in standalone route mode", async () => {
    renderWithI18n(<IntegrationsTab standalone />, { lobe: true });

    // The heading is the tab's own again: `SettingsTab`'s non-nested branch
    // used to render it, and that component is gone (inside the dialog the
    // shell's `DialogHeader` owns the title, which is why dropping it there is
    // correct). The standalone Web route has no shell, so it keeps one.
    expect(await screen.findByRole("heading", { name: "IM" })).toBeInTheDocument();
    expect(screen.getByTestId("integration-channel-card-lark")).toBeInTheDocument();
  });
});
