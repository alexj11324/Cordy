// @vitest-environment jsdom

import { afterEach, describe, expect, it } from "vitest";
import { cleanup, render, screen } from "@testing-library/react";
import "@testing-library/jest-dom/vitest";
import { I18nProvider } from "@patchbay/core/i18n/react";
import enSettings from "../../locales/en/settings.json";
import zhSettings from "../../locales/zh-Hans/settings.json";
import jaSettings from "../../locales/ja/settings.json";
import koSettings from "../../locales/ko/settings.json";
import { MessagingConnectionStatus } from "./messaging-connection-status";

afterEach(cleanup);

describe("MessagingConnectionStatus", () => {
  it.each([
    ["en", enSettings, "installed", "added", "Bot installed."],
    ["zh-Hans", zhSettings, "已安装", "已添加", "机器人已安装。"],
    ["ja", jaSettings, "登録しました", "追加しました", "ボットを登録しました。"],
    ["ko", koSettings, "설치되었어요", "추가했어요", "봇이 설치되었어요."],
  ] as const)("does not promise a live connection after installation in %s", (_, copy, installed, added, complete) => {
    expect(copy.lark.install_success).toBe(complete);
    expect(copy.slack.byo_success_toast).toContain(installed);
    expect(copy.dingtalk.byo_success_toast).toContain(installed);
    expect(copy.wecom.byo_success_toast).toContain(installed);
    expect(copy.telegram.connect_success_toast).toContain(installed);
    expect(copy.weixin.install_success).toContain(added);
    expect(copy.weixin.install_success_toast).toContain(added);
  });

  it.each([
    ["en", enSettings, "Authorized", "Sync enabled for 2 projects", "Experimental"],
    ["zh-Hans", zhSettings, "已授权", "已为 2 个项目启用同步", "实验性"],
    ["ja", jaSettings, "認証済み", "2 件のプロジェクトで同期を有効化", "実験的"],
    ["ko", koSettings, "인증됨", "프로젝트 2개에서 동기화 활성화됨", "실험적"],
  ] as const)(
    "keeps authorization, sync enablement, and experimental maturity separate in %s",
    (_, copy, authorization, enabledSync, experimental) => {
      expect(copy.page.linear.healthy).toBe(authorization);
      expect(copy.page.linear.projects_synced.replace("{{count}}", "2")).toBe(enabledSync);
      expect(copy.page.connection_status.experimental).toBe(experimental);
    },
  );

  it("renders the requested connection terminology with an accessible state", () => {
    render(
      <I18nProvider
        locale="zh-Hans"
        resources={{ "zh-Hans": { settings: zhSettings } }}
      >
        <MessagingConnectionStatus
          installation={{
            status: "installed",
            runtime: {
              state: "healthy",
              observedAt: "2026-09-03T10:00:00Z",
              errorCode: null,
            },
          }}
        />
      </I18nProvider>,
    );
    expect(screen.getByRole("status", { name: "连接状态" })).toHaveTextContent(
      "已连接",
    );
    expect(screen.queryByText("健康")).toBeNull();
  });

  it("shows missing observations as unknown and never renders provider diagnostics", () => {
    render(
      <I18nProvider locale="en" resources={{ en: { settings: enSettings } }}>
        <MessagingConnectionStatus
          installation={{
            status: "installed",
            runtime: {
              state: "future_state",
              observedAt: null,
              errorCode: "future_code",
              errorSummary: "credential-sentinel",
            },
          }}
        />
      </I18nProvider>,
    );
    expect(
      screen.getByRole("status", { name: "Connection status" }),
    ).toHaveTextContent("Status unavailable");
    expect(screen.queryByText("credential-sentinel")).toBeNull();
  });

  it("shows experimental maturity without replacing confirmed connectivity", () => {
    render(
      <I18nProvider locale="en" resources={{ en: { settings: enSettings } }}>
        <MessagingConnectionStatus
          installation={{
            status: "installed",
            runtime: {
              state: "healthy",
              observedAt: "2026-09-03T10:00:00Z",
              errorCode: null,
            },
            setup: { mode: "managed_token", writable: true, experimental: true },
          }}
        />
      </I18nProvider>,
    );
    const status = screen.getByRole("status", { name: "Connection status" });
    expect(status).toHaveTextContent("Connected");
    expect(status).toHaveTextContent("Experimental");
  });
});
