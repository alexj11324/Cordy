// @vitest-environment jsdom

import { type ReactNode } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { I18nProvider } from "@patchbay/core/i18n/react";
import enCommon from "../../locales/en/common.json";
import enSettings from "../../locales/en/settings.json";

type MemberRole = "owner" | "admin" | "member" | "guest";

const membersRef = vi.hoisted(() => ({
  current: [{ user_id: "user-1", role: "owner" as MemberRole }] as unknown[],
}));
const agentsRef = vi.hoisted(() => ({
  current: [
    { id: "agent-1", name: "Planner", owner_id: "user-1", archived_at: null },
    { id: "agent-archived", name: "Archived", owner_id: "user-1", archived_at: "2026-01-01" },
  ] as unknown[],
}));
const installationsRef = vi.hoisted(() => ({
  current: {
    installations: [] as unknown[],
    configured: true,
    install_supported: true,
  },
}));
const installationsLoadingRef = vi.hoisted(() => ({ current: false }));
const installationsErrorRef = vi.hoisted(() => ({ current: false }));
const mockBegin = vi.hoisted(() => vi.fn());
const mockStatus = vi.hoisted(() => vi.fn());
const mockDelete = vi.hoisted(() => vi.fn());
const mockInvalidate = vi.hoisted(() => vi.fn());
const mockToastSuccess = vi.hoisted(() => vi.fn());
const mockToastError = vi.hoisted(() => vi.fn());
const MockApiError = vi.hoisted(() =>
  class MockApiError extends Error {
    readonly status: number;

    constructor(message: string, status: number) {
      super(message);
      this.name = "ApiError";
      this.status = status;
    }
  },
);

vi.mock("@tanstack/react-query", () => ({
  useQuery: (opts: { queryKey: unknown[]; enabled?: boolean }) => {
    if (opts.enabled === false) return { data: undefined, isLoading: false, isError: false };
    const key = JSON.stringify(opts.queryKey);
    if (key.includes("members")) return { data: membersRef.current, isLoading: false, isError: false };
    if (key.includes("agents")) return { data: agentsRef.current, isLoading: false, isError: false };
    if (key.includes("weixin")) {
      return {
        data: installationsLoadingRef.current ? undefined : installationsRef.current,
        isLoading: installationsLoadingRef.current,
        isError: installationsErrorRef.current,
      };
    }
    return { data: undefined, isLoading: false, isError: false };
  },
  useQueryClient: () => ({ invalidateQueries: mockInvalidate }),
  queryOptions: <T,>(opts: T) => opts,
}));

vi.mock("@patchbay/core/hooks", () => ({ useWorkspaceId: () => "workspace-1" }));

vi.mock("@patchbay/core/workspace/queries", () => ({
  memberListOptions: () => ({ queryKey: ["members"], queryFn: vi.fn() }),
  agentListOptions: () => ({ queryKey: ["agents"], queryFn: vi.fn() }),
}));

vi.mock("@patchbay/core/weixin", () => ({
  weixinInstallationsOptions: () => ({
    queryKey: ["weixin", "workspace-1", "installations"],
    queryFn: vi.fn(),
  }),
  weixinKeys: {
    installations: (wsId: string) => ["weixin", wsId, "installations"],
  },
}));

vi.mock("@patchbay/core/api", () => ({
  api: {
    beginWeixinInstall: mockBegin,
    getWeixinInstallStatus: mockStatus,
    deleteWeixinInstallation: mockDelete,
  },
  ApiError: MockApiError,
}));

vi.mock("@patchbay/core/auth", () => {
  const useAuthStore = Object.assign(
    (selector?: (state: { user: { id: string } }) => unknown) =>
      selector ? selector({ user: { id: "user-1" } }) : { user: { id: "user-1" } },
    { getState: () => ({ user: { id: "user-1" } }) },
  );
  return { useAuthStore };
});

vi.mock("react-qr-code", () => ({
  QRCode: ({ value }: { value: string }) => <svg data-testid="weixin-qr" data-value={value} />,
}));

vi.mock("sonner", () => ({
  toast: { success: mockToastSuccess, error: mockToastError, message: vi.fn() },
}));

import { WeixinTab } from "./weixin-tab";

const TEST_RESOURCES = { en: { common: enCommon, settings: enSettings } };

afterEach(cleanup);

function renderUI(children: ReactNode) {
  return render(
    <I18nProvider locale="en" resources={TEST_RESOURCES}>
      {children}
    </I18nProvider>,
  );
}

function resetFixtures() {
  vi.clearAllMocks();
  membersRef.current = [{ user_id: "user-1", role: "owner" }];
  agentsRef.current = [
    { id: "agent-1", name: "Planner", owner_id: "user-1", archived_at: null },
    { id: "agent-archived", name: "Archived", owner_id: "user-1", archived_at: "2026-01-01" },
  ];
  installationsRef.current = { installations: [], configured: true, install_supported: true };
  installationsLoadingRef.current = false;
  installationsErrorRef.current = false;
  mockInvalidate.mockResolvedValue(undefined);
}

describe("WeixinTab", () => {
  beforeEach(resetFixtures);

  it("starts a Personal Weixin session and renders its QR code", async () => {
    mockBegin.mockResolvedValue({
      session_id: "session-1",
      qr_code_url: "weixin://qr/personal-1",
      expires_in_seconds: 300,
      poll_interval_seconds: 30,
    });
    const user = userEvent.setup();
    renderUI(<WeixinTab />);

    await user.click(screen.getByTestId("weixin-connect-agent-agent-1"));
    await waitFor(() => expect(mockBegin).toHaveBeenCalledWith("workspace-1", "agent-1"));
    expect(await screen.findByTestId("weixin-qr")).toHaveAttribute(
      "data-value",
      "weixin://qr/personal-1",
    );
    expect(screen.getByRole("link", { name: /authorization link/i })).toHaveAttribute(
      "href",
      "weixin://qr/personal-1",
    );
  });

  it("polls for the verification-code state and redeems the trimmed code", async () => {
    mockBegin.mockResolvedValue({
      session_id: "session-verify",
      qr_code_url: "weixin://qr/verify",
      expires_in_seconds: 300,
      poll_interval_seconds: 0,
    });
    mockStatus
      .mockResolvedValueOnce({ status: "need_verify_code" })
      .mockResolvedValueOnce({ status: "success", installation_id: "installation-1" });
    const user = userEvent.setup();
    renderUI(<WeixinTab />);

    await user.click(screen.getByTestId("weixin-connect-agent-agent-1"));
    const codeInput = await screen.findByTestId("weixin-verify-code", {}, { timeout: 4000 });
    await user.type(codeInput, " 2468 ");
    await user.click(screen.getByRole("button", { name: /continue/i }));

    await waitFor(() =>
      expect(mockStatus).toHaveBeenLastCalledWith("workspace-1", "session-verify", "2468"),
    );
    await waitFor(() => expect(mockInvalidate).toHaveBeenCalledWith({
      queryKey: ["weixin", "workspace-1", "installations"],
    }));
    expect(mockToastSuccess).toHaveBeenCalled();
  }, 7000);

  it("revokes an active installation only after confirmation", async () => {
    installationsRef.current = {
      installations: [{ id: "installation-1", agent_id: "agent-1", bot_id: "personal-bot", status: "active" }],
      configured: true,
      install_supported: true,
    };
    mockDelete.mockResolvedValue(undefined);
    const user = userEvent.setup();
    renderUI(<WeixinTab />);

    await user.click(screen.getByRole("button", { name: /^Disconnect$/i }));
    expect(mockDelete).not.toHaveBeenCalled();
    const confirmButtons = await screen.findAllByRole("button", { name: /^Disconnect$/i });
    await user.click(confirmButtons.at(-1)!);
    await waitFor(() =>
      expect(mockDelete).toHaveBeenCalledWith("workspace-1", "installation-1"),
    );
    expect(mockInvalidate).toHaveBeenCalledWith({
      queryKey: ["weixin", "workspace-1", "installations"],
    });
  });

  it("renders unknown installation statuses as a safe non-active fallback", () => {
    installationsRef.current = {
      installations: [{ id: "installation-unknown", agent_id: "agent-1", bot_id: "bot", status: "future_status" }],
      configured: true,
      install_supported: true,
    };
    renderUI(<WeixinTab />);

    expect(screen.getByText(/^revoked$/i)).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /^Disconnect$/i })).toBeNull();
  });
});
