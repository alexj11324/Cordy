// @vitest-environment jsdom

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import "@testing-library/jest-dom/vitest";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { ApiError } from "@patchbay/core/api";
import { configStore } from "@patchbay/core/config";
import { COMPOSIO_MCP_APPS_FLAG } from "@patchbay/core/feature-flags";
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
const dingtalkInstallationsRef = vi.hoisted(() => ({
  current: undefined as
    | {
        configured: boolean;
        install_supported: boolean;
        installations: { id: string; agent_id: string | null; status: string }[];
      }
    | undefined,
}));
const larkInstallationsRef = vi.hoisted(() => ({
  current: undefined as
    | {
        configured: boolean;
        install_supported: boolean;
        installations: {
          id: string;
          agent_id: string | null;
          status: string;
          region?: string;
        }[];
      }
    | undefined,
}));

vi.mock("@tanstack/react-query", () => ({
  useQuery: (opts: { queryKey: unknown[]; enabled?: boolean }) => {
    queryCallsRef.current.push(opts);
    const isMemberQuery = opts.queryKey[opts.queryKey.length - 1] === "members";
    const isDingTalkInstallationsQuery =
      opts.queryKey[0] === "dingtalk" &&
      opts.queryKey[opts.queryKey.length - 1] === "installations";
    const isLarkInstallationsQuery =
      opts.queryKey[0] === "lark" &&
      opts.queryKey[opts.queryKey.length - 1] === "installations";
    return {
      data: isMemberQuery
        ? membersRef.current
        : isDingTalkInstallationsQuery
          ? dingtalkInstallationsRef.current
          : isLarkInstallationsQuery
            ? larkInstallationsRef.current
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

vi.mock("@patchbay/core/hooks", () => ({
  useWorkspaceId: () => "workspace-id",
}));

vi.mock("@patchbay/core/auth", () => ({
  useAuthStore: (selector: (state: { user: typeof authUserRef.current }) => unknown) =>
    selector({ user: authUserRef.current }),
}));

vi.mock("./lark-tab", () => ({
  LarkTab: () => <div data-testid="lark-tab" />,
  LarkAgentBindButton: () => <button>Connect Lark</button>,
}));

vi.mock("./composio-tab", () => ({
  ComposioTab: () => <div data-testid="composio-tab" />,
}));

vi.mock("./slack-tab", () => ({
  SlackTab: () => <div data-testid="slack-tab" />,
  SlackAgentBindButton: () => <button>Connect Slack</button>,
}));

vi.mock("./dingtalk-tab", () => ({
  DingTalkTab: () => <div data-testid="dingtalk-tab" />,
  DingTalkAgentBindButton: () => <button>Connect DingTalk</button>,
}));

vi.mock("./vcs-tab", () => ({
  VCSTab: () => <div data-testid="vcs-tab" />,
}));

vi.mock("./wecom-tab", () => ({
  WecomTab: () => <div data-testid="wecom-tab" />,
  WecomAgentBindButton: () => <button>Connect WeCom</button>,
}));

vi.mock("./telegram-tab", () => ({
  TelegramTab: () => <div data-testid="telegram-tab" />,
  TelegramAgentBindButton: () => <button>Connect Telegram</button>,
}));

vi.mock("./weixin-tab", () => ({
  WeixinTab: () => <div data-testid="weixin-tab" />,
  WeixinAgentBindButton: () => <button>Connect WeChat</button>,
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
    dingtalkInstallationsRef.current = undefined;
    larkInstallationsRef.current = undefined;
    configStore.getState().setFeatureFlags({ [COMPOSIO_MCP_APPS_FLAG]: true });
    // Reset the self-host-only VCS gate to its default (hidden) so tests stay
    // isolated; individual tests opt in below.
    configStore.getState().setAuthConfig({ allowSignup: true, vcsIntegrationAvailable: false });
  });

  it("hides Composio and disables the toolkits query when the feature flag is off", () => {
    configStore.getState().setFeatureFlags({ [COMPOSIO_MCP_APPS_FLAG]: false });

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

  it("shows each channel description below its icon and title", () => {
    renderTab();

    for (const channel of ["weixin", "lark", "slack", "dingtalk", "wecom", "telegram"]) {
      const card = screen.getByTestId(`integration-channel-card-${channel}`);
      const icon = screen.getByTestId(`integration-channel-icon-${channel}`);
      const title = card.querySelector("h3");
      const description = title?.nextElementSibling;
      expect(title).not.toBeNull();
      expect(description?.tagName).toBe("P");
      expect(description).toHaveClass("text-caption", "text-muted-foreground");
      expect(card.parentElement).toHaveClass("grid", "md:grid-cols-2", "xl:grid-cols-3");
      expect(icon).not.toHaveClass("border");
      expect(icon).toHaveClass("size-12");
    }
  });

  it("shows connected Hub management actions without an Agent preselection", () => {
    authUserRef.current = { id: "admin-user" };
    membersRef.current = [{ user_id: "admin-user", role: "owner" }];
    dingtalkInstallationsRef.current = {
      configured: true,
      install_supported: true,
      installations: [{ id: "hub-1", agent_id: null, status: "active" }],
    };

    renderTab();

    expect(screen.getByText("Connected")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Manage" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Reconnect" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Disconnect" })).toBeInTheDocument();
  });

  it("opens the real provider setup panel when the deployment has not enabled it", () => {
    authUserRef.current = { id: "admin-user" };
    membersRef.current = [{ user_id: "admin-user", role: "owner" }];
    dingtalkInstallationsRef.current = {
      configured: false,
      install_supported: false,
      installations: [],
    };

    renderTab();

    expect(screen.getByText("Not enabled on this deployment")).toBeInTheDocument();
    expect(screen.queryByText("Not configured")).toBeNull();

    fireEvent.click(screen.getByRole("button", { name: "Configure" }));

    expect(screen.getByRole("heading", { name: "Platform setup" })).toBeInTheDocument();
    expect(screen.getByTestId("dingtalk-tab")).toBeInTheDocument();
  });

  it("hides reconnect for an active international Lark Hub while that flow is disabled", () => {
    authUserRef.current = { id: "admin-user" };
    membersRef.current = [{ user_id: "admin-user", role: "owner" }];
    larkInstallationsRef.current = {
      configured: true,
      install_supported: true,
      installations: [{ id: "lark-hub", agent_id: null, status: "active", region: "lark" }],
    };

    renderTab();

    expect(screen.getByRole("button", { name: "Manage" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Disconnect" })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Reconnect" })).toBeNull();
  });

  it("explains that Agent selection happens in the connected chat", () => {
    renderTab();

    expect(
      screen.getByText(
        "Connect a platform once, then use /agents in the chat to choose which Agent handles each conversation.",
      ),
    ).toBeInTheDocument();
    expect(screen.getAllByText("Workspace admin only")).toHaveLength(6);
  });

  it("shows a login gate for guests instead of an external connection action", () => {
    authUserRef.current = { id: "guest-user", is_guest: true };
    membersRef.current = [{ user_id: "guest-user", role: "owner" }];

    renderTab();

    expect(screen.getAllByText("Log in to connect")).toHaveLength(6);
    expect(screen.queryByText("Workspace admin only")).toBeNull();
  });

  // Reaching for a generic lucide glyph is how Slack and WeCom ended up sharing
  // one speech bubble, with nothing on the row saying which platform it was
  // (#6585). Requiring distinct shapes is the cheap guard against a
  // regression to that.
  it("gives every channel its own brand mark", () => {
    renderTab();

    const shapes = ["weixin", "lark", "slack", "dingtalk", "wecom", "telegram"].map(
      (channel) => screen.getByTestId(`integration-channel-icon-${channel}`).innerHTML,
    );

    expect(new Set(shapes).size).toBe(shapes.length);
  });

  it("keeps legacy DingTalk route management available", () => {
    dingtalkInstallationsRef.current = {
      configured: true,
      install_supported: true,
      installations: [{ id: "legacy-1", agent_id: "legacy-agent", status: "active" }],
    };

    renderTab();

    expect(screen.getByText("Legacy DingTalk routing")).toBeInTheDocument();
    expect(screen.getByTestId("dingtalk-tab")).toBeInTheDocument();
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
});
