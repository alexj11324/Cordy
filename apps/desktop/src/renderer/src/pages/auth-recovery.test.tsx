// @vitest-environment jsdom

import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import enAuth from "../../../../../../packages/views/locales/en/auth.json";
import zhHansAuth from "../../../../../../packages/views/locales/zh-Hans/auth.json";
import jaAuth from "../../../../../../packages/views/locales/ja/auth.json";
import koAuth from "../../../../../../packages/views/locales/ko/auth.json";

type RecoveryTranslation = {
  desktop: {
    recovery: {
      title: string;
      description: string;
      retry: string;
      retrying: string;
      daemon: Record<string, string>;
    };
  };
};

const localeState = vi.hoisted(() => ({
  current: "en" as "en" | "zh-Hans" | "ja" | "ko",
  messages: {
    en: {
      desktop: {
        recovery: {
          title: "Reconnecting to Patchbay",
          description: "Your session is still saved.",
          retry: "Try again",
          retrying: "Trying again...",
          daemon: {
            session_token_missing: "The Desktop session token is missing.",
            auto_start_disabled: "Automatic local daemon startup is disabled.",
            cli_not_found: "The source-matched Patchbay CLI was not found.",
            auth_expired: "The local daemon session expired.",
            not_ready: "The local daemon is not ready.",
            start_failed: "The local daemon could not start.",
          },
        },
      },
    },
    "zh-Hans": {
      desktop: {
        recovery: {
          title: "正在重新连接 Patchbay",
          description: "登录状态已保留。",
          retry: "重试",
          retrying: "正在重试...",
          daemon: {
            session_token_missing: "桌面会话令牌缺失。",
            auto_start_disabled: "本地守护进程自动启动已禁用。",
            cli_not_found: "找不到与当前源码匹配的 Patchbay CLI。",
            auth_expired: "本地守护进程会话已过期。",
            not_ready: "本地守护进程尚未就绪。",
            start_failed: "本地守护进程无法启动。",
          },
        },
      },
    },
    ja: {
      desktop: {
        recovery: {
          title: "Patchbay に再接続しています",
          description: "セッションは保持されています。",
          retry: "再試行",
          retrying: "再試行しています...",
          daemon: {
            session_token_missing: "デスクトップセッション トークンがありません。",
            auto_start_disabled: "ローカルデーモンの自動起動が無効です。",
            cli_not_found: "ソースに一致する Patchbay CLI が見つかりません。",
            auth_expired: "ローカルデーモンのセッションが期限切れです。",
            not_ready: "ローカルデーモンの準備ができていません。",
            start_failed: "ローカルデーモンを起動できませんでした。",
          },
        },
      },
    },
    ko: {
      desktop: {
        recovery: {
          title: "Patchbay에 다시 연결하는 중",
          description: "세션은 안전하게 유지됩니다.",
          retry: "다시 시도",
          retrying: "다시 시도하는 중...",
          daemon: {
            session_token_missing: "데스크톱 세션 토큰이 없습니다.",
            auto_start_disabled: "로컬 데몬 자동 시작이 비활성화되어 있습니다.",
            cli_not_found: "소스와 일치하는 Patchbay CLI를 찾을 수 없습니다.",
            auth_expired: "로컬 데몬 세션이 만료되었습니다.",
            not_ready: "로컬 데몬이 아직 준비되지 않았습니다.",
            start_failed: "로컬 데몬을 시작하지 못했습니다.",
          },
        },
      },
    },
  },
}));

vi.mock("@patchbay/core/auth", () => ({
  useAuthStore: (
    selector: (state: { retryAuthentication: () => void }) => unknown,
  ) => selector({ retryAuthentication: vi.fn() }),
}));

vi.mock("@patchbay/ui/components/common/patchbay-icon", () => ({
  PatchbayIcon: () => <div data-testid="patchbay-icon" />,
}));

vi.mock("@patchbay/views/platform", () => ({
  DragStrip: () => null,
}));

vi.mock("@patchbay/views/i18n", () => ({
  useT: () => ({
    t: (select: (value: RecoveryTranslation) => unknown) =>
      select(localeState.messages[localeState.current]),
  }),
}));

import { DesktopAuthRecoveryPage } from "./auth-recovery";

afterEach(cleanup);

function renderRecovery(locale: "en" | "zh-Hans" | "ja" | "ko") {
  localeState.current = locale;
  return render(<DesktopAuthRecoveryPage errorReason="cli_not_found" />);
}

describe("DesktopAuthRecoveryPage", () => {
  it("renders an actionable daemon diagnostic in a non-English locale", async () => {
    renderRecovery("zh-Hans");

    const alert = await screen.findByRole("alert");
    expect(alert).toHaveTextContent("找不到与当前源码匹配的 Patchbay CLI");
    expect(alert).not.toHaveTextContent("desktop.recovery.daemon.cli_not_found");
  });

  it.each([
    ["en", enAuth.desktop.recovery.daemon.cli_not_found],
    ["zh-Hans", zhHansAuth.desktop.recovery.daemon.cli_not_found],
    ["ja", jaAuth.desktop.recovery.daemon.cli_not_found],
    ["ko", koAuth.desktop.recovery.daemon.cli_not_found],
  ] as const)("keeps the CLI-missing resource translated for %s", (locale, message) => {
    expect(message).not.toContain("desktop.recovery.daemon");
    expect(message).toBeTruthy();
    renderRecovery(locale);
    expect(screen.getByRole("alert")).toHaveTextContent(
      localeState.messages[locale].desktop.recovery.daemon.cli_not_found,
    );
  });
});
