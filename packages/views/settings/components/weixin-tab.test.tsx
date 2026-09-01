// @vitest-environment jsdom

import { type ReactNode } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { I18nProvider } from "@patchbay/core/i18n/react";
import enCommon from "../../locales/en/common.json";
import enSettings from "../../locales/en/settings.json";

const mockBeginInstall = vi.hoisted(() => vi.fn());
const mockGetStatus = vi.hoisted(() => vi.fn());
const mockInvalidate = vi.hoisted(() => vi.fn());

vi.mock("@tanstack/react-query", () => ({
  useQuery: () => ({
    data: {
      configured: true,
      install_supported: true,
      installations: [],
    },
    isLoading: false,
  }),
  useQueryClient: () => ({ invalidateQueries: mockInvalidate }),
  queryOptions: <T,>(options: T) => options,
}));

vi.mock("@patchbay/core/hooks", () => ({
  useWorkspaceId: () => "workspace-1",
}));

vi.mock("@patchbay/core/weixin", () => ({
  weixinInstallationsOptions: () => ({
    queryKey: ["weixin", "installations", "workspace-1"],
  }),
  weixinKeys: {
    installations: (workspaceId: string) => [
      "weixin",
      "installations",
      workspaceId,
    ],
  },
}));

vi.mock("@patchbay/core/api", () => ({
  api: {
    beginWeixinInstall: mockBeginInstall,
    getWeixinInstallStatus: mockGetStatus,
    deleteWeixinInstallation: vi.fn(),
  },
}));

vi.mock("sonner", () => ({
  toast: { success: vi.fn(), error: vi.fn() },
}));

vi.mock("react-qr-code", () => ({
  QRCode: ({ value }: { value: string }) => (
    <span data-testid="weixin-qr-code" data-value={value} />
  ),
}));

import { WeixinAgentBindButton } from "./weixin-tab";

function renderUI(children: ReactNode) {
  return render(
    <I18nProvider
      locale="en"
      resources={{ en: { common: enCommon, settings: enSettings } }}
    >
      {children}
    </I18nProvider>,
  );
}

describe("WeixinAgentBindButton", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    mockBeginInstall.mockResolvedValue({
      session_id: "session-1",
      qr_code_url: "https://ilink.weixin.qq.com/qr/session-1",
      poll_interval_seconds: 60,
    });
  });

  afterEach(cleanup);

  it("generates Tencent's QR code inside the current settings page", async () => {
    renderUI(<WeixinAgentBindButton />);

    await userEvent.click(
      screen.getByRole("button", { name: "Connect WeChat" }),
    );

    await waitFor(() =>
      expect(mockBeginInstall).toHaveBeenCalledWith("workspace-1", undefined),
    );
    expect(await screen.findByTestId("weixin-qr-code")).toHaveAttribute(
      "data-value",
      "https://ilink.weixin.qq.com/qr/session-1",
    );
    expect(screen.getByText("Scan with WeChat")).toBeInTheDocument();
  });
});
