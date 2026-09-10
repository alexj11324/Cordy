import { describe, expect, it, vi } from "vitest";
import { render, screen } from "@testing-library/react";

import type { DaemonStatus } from "../../../shared/daemon-types";

const translations = {
  desktop: {
    daemon: {
      start: "启动",
      restart: "重启守护进程",
      stop: "终止守护进程",
    },
  },
};

// The component only needs these to render; stub them so the test focuses on
// the externally-managed branching, not data fetching.
vi.mock("@tanstack/react-query", () => ({
  useQuery: () => ({ data: [] }),
}));
vi.mock("@orvilo/core/hooks", () => ({
  useWorkspaceId: () => "ws-1",
}));
vi.mock("@orvilo/core/runtimes", () => ({
  runtimeListOptions: () => ({ queryKey: ["runtimes"] }),
}));
vi.mock("@orvilo/core/agents", () => ({
  agentTaskSnapshotOptions: () => ({ queryKey: ["snapshot"] }),
}));
vi.mock("@orvilo/views/i18n", () => ({
  useT: () => ({
    t: (selector: (resources: typeof translations) => string) =>
      selector(translations),
  }),
}));
vi.mock("../platform/daemon-reauth", () => ({
  reauthenticateDaemon: vi.fn(),
}));
vi.mock("sonner", () => ({
  toast: { error: vi.fn(), success: vi.fn() },
}));

import { DaemonRuntimeActions } from "./daemon-runtime-card";

function stubDaemonAPI(status: DaemonStatus) {
  Object.defineProperty(window, "daemonAPI", {
    configurable: true,
    value: {
      getStatus: vi.fn().mockResolvedValue(status),
      onStatusChange: vi.fn(() => () => {}),
    },
  });
}

describe("DaemonRuntimeActions — externally managed daemon (#3916)", () => {
  it("hides controls for a daemon the app can't control", async () => {
    stubDaemonAPI({ state: "running", daemonId: "d1", externallyManaged: true });
    render(<DaemonRuntimeActions />);

    expect(screen.queryByText("由应用外部管理")).not.toBeInTheDocument();
    expect(screen.queryByText("重启守护进程")).not.toBeInTheDocument();
    expect(screen.queryByText("终止守护进程")).not.toBeInTheDocument();
    expect(screen.queryByText("查看日志")).not.toBeInTheDocument();
  });

  it("shows Stop/Restart for a normally-managed running daemon (no 误伤)", async () => {
    stubDaemonAPI({
      state: "running",
      daemonId: "d1",
      externallyManaged: false,
    });
    render(<DaemonRuntimeActions />);

    expect(await screen.findByText("重启守护进程")).toBeInTheDocument();
    expect(screen.getByText("终止守护进程")).toBeInTheDocument();
    expect(
      screen.queryByText("由应用外部管理"),
    ).not.toBeInTheDocument();
  });
});

describe("DaemonRuntimeActions — recovery budget", () => {
  it("offers a manual Start when automatic recovery is paused", async () => {
    stubDaemonAPI({ state: "recovery_paused" });
    render(<DaemonRuntimeActions />);

    expect(await screen.findByText("启动")).toBeInTheDocument();
  });
});
