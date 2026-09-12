import { act, render, screen } from "@testing-library/react";
import type { ReactNode } from "react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { I18nProvider } from "@orvilo/core/i18n/react";
import { LobeThemeBridge } from "@orvilo/views/lobe";
import { RESOURCES } from "@orvilo/views/locales";
import type { DaemonStatus } from "../../../shared/daemon-types";
import { DaemonPanel } from "./daemon-panel";
import { DaemonSettingsTab } from "./daemon-settings-tab";

// See the note in `updates-settings-tab.test.tsx`: mounting `LobeThemeBridge`
// is the expensive part (antd's cssinjs runtime builds the component stylesheet
// on first render), measured at 8.4s here inside a full `vitest run` against a
// 5s default. The budget is widened rather than the assertion loosened — this
// test's first query waits on a render it cannot make faster.
const LOBE_MOUNT_TIMEOUT_MS = 20_000;
vi.setConfig({ testTimeout: LOBE_MOUNT_TIMEOUT_MS });

vi.mock("sonner", () => ({
  toast: { error: vi.fn(), success: vi.fn() },
}));

vi.mock("../platform/daemon-reauth", () => ({
  reauthenticateDaemon: vi.fn(),
}));

let emitLogLine: (line: string) => void = () => {};

function installDaemonAPI(status: DaemonStatus) {
  Object.defineProperty(window, "daemonAPI", {
    configurable: true,
    value: {
      getPrefs: vi.fn().mockResolvedValue({ autoStart: true, autoStop: false }),
      setPrefs: vi.fn().mockResolvedValue({ autoStart: true, autoStop: false }),
      isCliInstalled: vi.fn().mockResolvedValue(true),
      getStatus: vi.fn().mockResolvedValue(status),
      onStatusChange: vi.fn(() => () => {}),
      startLogStream: vi.fn(),
      stopLogStream: vi.fn(),
      onLogLine: vi.fn((handler: (line: string) => void) => {
        emitLogLine = handler;
        return () => {};
      }),
    },
  });
}

// `LobeThemeBridge` wraps both surfaces: `DaemonSettingsTab` is on the Lobe
// settings shell, and `DaemonPanel` sits inside the same settings dialog at
// runtime. A Lobe control calls `useMotionComponent()`, which throws without a
// `MotionProvider`, and the bridge is where `SettingsPage` supplies one.
function renderInSimplifiedChinese(element: ReactNode) {
  return render(
    <LobeThemeBridge>
      <I18nProvider locale="zh-Hans" resources={RESOURCES}>
        {element}
      </I18nProvider>
    </LobeThemeBridge>,
  );
}

beforeEach(() => {
  emitLogLine = () => {};
});

describe("Desktop daemon localization with real zh-Hans resources", () => {
  it("lets the locale own the externally managed sentence ending", async () => {
    installDaemonAPI({ state: "running", externallyManaged: true });

    renderInSimplifiedChinese(<DaemonSettingsTab />);

    expect(
      await screen.findByText(
        "登录时启动守护进程。应用打开期间，它会同时监控自动启动和手动启动的守护进程。",
      ),
    ).toBeInTheDocument();
    const command = screen.getByText("orvilo daemon stop");
    expect(command.closest("p")).toHaveTextContent(/orvilo daemon stop。$/);
  });

  it("renders repeated log messages with straight double quotes", async () => {
    installDaemonAPI({ state: "running" });
    renderInSimplifiedChinese(
      <DaemonPanel
        open
        onOpenChange={vi.fn()}
        status={{ state: "running" }}
        runtimeCount={0}
      />,
    );

    await act(async () => {
      emitLogLine("12:00:00.000 INF poll complete component=daemon");
      emitLogLine("12:00:01.000 INF poll complete component=daemon");
    });

    expect(
      await screen.findByText('另有 1 条"poll complete"——点击展开'),
    ).toBeInTheDocument();
  });
});
