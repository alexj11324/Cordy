// @vitest-environment jsdom

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import "@testing-library/jest-dom/vitest";
import { cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { ApiError } from "@patchbay/core/api";
import { configStore } from "@patchbay/core/config";
import {
  COMPOSIO_MCP_APPS_FLAG,
  LINEAR_INSTALLATION_FOUNDATION_FLAG,
} from "@patchbay/core/feature-flags";
import { I18nProvider } from "@patchbay/core/i18n/react";
import enCommon from "../../locales/en/common.json";
import enSettings from "../../locales/en/settings.json";

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
const channelInstallationsRef = vi.hoisted(() => ({
  current: {} as Partial<Record<
    "lark" | "slack" | "dingtalk" | "wecom" | "telegram" | "weixin",
    {
      configured: boolean;
      install_supported: boolean;
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
    const isChannelInstallationsQuery =
      typeof channel === "string" &&
      channel in channelInstallationsRef.current &&
      opts.queryKey[opts.queryKey.length - 1] === "installations";
    return {
      data: isMemberQuery
        ? membersRef.current
        : isMessagingQuotaQuery
          ? messagingQuotaRef.current
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

vi.mock("@patchbay/core/composio", () => ({
  composioToolkitsOptions: () => ({ queryKey: ["composio", "toolkits"] }),
}));

vi.mock("@patchbay/core/paths", () => ({
  useCurrentWorkspace: () => ({ id: "workspace-1", name: "Acme", slug: "acme" }),
}));

vi.mock("@patchbay/core/auth", () => ({
  useAuthStore: (selector: (state: { user: typeof authUserRef.current }) => unknown) =>
    selector({ user: authUserRef.current }),
}));

for (const channel of ["lark", "slack", "dingtalk", "wecom", "telegram", "weixin"]) {
  vi.doMock(`@patchbay/core/${channel}`, () => ({
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

function renderTab() {
  return render(
    <I18nProvider locale="en" resources={{ en: { common: enCommon, settings: enSettings } }}>
      <IntegrationsTab />
    </I18nProvider>,
  );
}

describe("Settings IntegrationsTab", () => {
  beforeEach(() => {
    queryCallsRef.current = [];
    composioErrorRef.current = null;
    authUserRef.current = null;
    membersRef.current = [];
    messagingQuotaRef.current = undefined;
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

  it("renders messaging integrations as workspace cards instead of expanded forms", () => {
    renderTab();

    for (const channel of ["lark", "slack", "dingtalk", "wecom", "telegram", "weixin"]) {
      expect(screen.getByTestId(`integration-channel-card-${channel}`)).toBeInTheDocument();
      expect(screen.queryByTestId(`${channel}-tab`)).toBeNull();
    }
  });

  it("shows used plus reserved hosted Agent turns", () => {
    messagingQuotaRef.current = { mode: "managed", used: 7, reserved: 2, limit: 100 };
    renderTab();
    expect(screen.getByTestId("messaging-quota")).toHaveTextContent(
      "9 of 100 Agent turns used this period",
    );
  });

  it("shows a runtime-confirmed workspace installation as connected", () => {
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

    renderTab();

    const card = screen.getByTestId("integration-channel-card-slack");
    expect(within(card).getByText("Connected")).toBeInTheDocument();
    expect(within(card).getByRole("button", { name: "Manage" })).toBeInTheDocument();
  });

  it("keeps server-configured messaging read-only", () => {
    authUserRef.current = { id: "admin-user" };
    membersRef.current = [{ user_id: "admin-user", role: "owner" }];
    configStore.getState().setMessagingConfig({
      mode: "server_configured",
      setupWritable: false,
      platforms: [],
    });

    renderTab();

    expect(screen.getAllByText("Configured by the server operator")).toHaveLength(6);
  });

  it("opens the platform setup guide without exposing deployment variables", () => {
    authUserRef.current = { id: "admin-user" };
    membersRef.current = [{ user_id: "admin-user", role: "owner" }];

    renderTab();
    const card = screen.getByTestId("integration-channel-card-dingtalk");
    fireEvent.click(within(card).getByRole("button", { name: "Configure" }));

    expect(screen.getByTestId("integration-setup-guide-dingtalk")).toBeInTheDocument();
    expect(screen.getByTestId("dingtalk-hub-install")).toBeInTheDocument();
    expect(screen.queryByText("PATCHBAY_DINGTALK_SECRET_KEY")).toBeNull();
  });

  it("hides Composio and disables the toolkits query when the feature flag is off", () => {    configStore.getState().setFeatureFlags({ [COMPOSIO_MCP_APPS_FLAG]: false });

    renderTab();

    expect(screen.queryByTestId("composio-tab")).toBeNull();
    const composioQuery = queryCallsRef.current.find(
      (query) => query.queryKey[0] === "composio",
    );
    expect(composioQuery?.enabled).toBe(false);
  });

  it("shows Composio when the feature flag is on and the integration is configured", () => {
    renderTab();

    expect(screen.getByTestId("composio-tab")).toBeInTheDocument();
    const composioQuery = queryCallsRef.current.find(
      (query) => query.queryKey[0] === "composio",
    );
    expect(composioQuery?.enabled).toBe(true);
  });

  it("shows Linear only when its installation feature is enabled", () => {
    renderTab();
    expect(screen.queryByTestId("integration-channel-card-linear")).toBeNull();

    cleanup();
    configStore.getState().setFeatureFlags({
      [COMPOSIO_MCP_APPS_FLAG]: true,
      [LINEAR_INSTALLATION_FOUNDATION_FLAG]: true,
    });
    renderTab();
    expect(screen.getByTestId("integration-channel-card-linear")).toBeInTheDocument();
  });

  it("shows each channel description below its icon and title", () => {
    renderTab();

    for (const channel of ["lark", "slack", "dingtalk", "wecom", "weixin", "telegram"]) {
      const card = screen.getByTestId(`integration-channel-card-${channel}`);
      const icon = screen.getByTestId(`integration-channel-icon-${channel}`);
      const title = card.querySelector("h3");
      const description = title?.nextElementSibling;
      expect(title).not.toBeNull();
      expect(description?.tagName).toBe("P");
      expect(description).toHaveClass("text-caption", "text-muted-foreground");
      expect(icon).not.toHaveClass("border");
      expect(icon).not.toHaveClass("bg-muted/40");
    }
  });

  // Reaching for a generic lucide glyph is how Slack and WeCom ended up sharing
  // one speech bubble, with nothing on the row saying which platform it was
  // (#6585). Requiring five distinct shapes is the cheap guard against a
  // regression to that.
  it("gives every channel its own brand mark", () => {
    renderTab();

    const shapes = ["lark", "slack", "dingtalk", "wecom", "weixin", "telegram"].map(
      (channel) => screen.getByTestId(`integration-channel-icon-${channel}`).innerHTML,
    );

    expect(new Set(shapes).size).toBe(shapes.length);
  });

  it("hides Composio when the feature flag is on but the server reports 503", () => {
    composioErrorRef.current = new ApiError("unavailable", 503, "Service Unavailable");

    renderTab();

    expect(screen.queryByTestId("composio-tab")).toBeNull();
  });

  it("hides the Git providers section when the deployment reports it unavailable", () => {
    // Default (managed cloud / older server): vcsIntegrationAvailable is false.
    renderTab();

    expect(screen.queryByTestId("vcs-tab")).toBeNull();
  });

  it("shows the Git providers section on a self-hosted deployment that enables it", () => {
    configStore.getState().setAuthConfig({ allowSignup: true, vcsIntegrationAvailable: true });

    renderTab();

    expect(screen.getByTestId("vcs-tab")).toBeInTheDocument();
  });

  it("renders the centered page chrome in standalone route mode", () => {
    render(
      <I18nProvider locale="en" resources={{ en: { common: enCommon, settings: enSettings } }}>
        <IntegrationsTab standalone />
      </I18nProvider>,
    );

    expect(screen.getByRole("heading", { name: "Integrations" })).toBeInTheDocument();
    expect(screen.getByTestId("integration-channel-card-lark")).toBeInTheDocument();
  });
});
