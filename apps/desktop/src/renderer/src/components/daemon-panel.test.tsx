// @vitest-environment jsdom
import { act, cleanup, render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import type { DaemonStatus } from "../../../shared/daemon-types";
import { DaemonPanel } from "./daemon-panel";

const status: DaemonStatus = { state: "running" };

describe("DaemonPanel credential reset", () => {
  let emitLine: ((line: string) => void) | undefined;
  let emitReset: (() => void) | undefined;

  afterEach(() => {
    cleanup();
    vi.restoreAllMocks();
    Reflect.deleteProperty(window, "daemonAPI");
  });

  it("clears already-rendered log history when the main process closes the gate", async () => {
    const daemonAPI = {
      startLogStream: vi.fn(),
      stopLogStream: vi.fn(),
      onLogLine: vi.fn((callback: (line: string) => void) => {
        emitLine = callback;
        return () => {};
      }),
      onLogReset: vi.fn((callback: () => void) => {
        emitReset = callback;
        return () => {};
      }),
    };
    Object.defineProperty(window, "daemonAPI", {
      configurable: true,
      value: daemonAPI,
    });

    render(
      <DaemonPanel
        open
        onOpenChange={() => {}}
        status={status}
        runtimeCount={0}
      />,
    );

    await waitFor(() =>
      expect(daemonAPI.startLogStream).toHaveBeenCalledOnce(),
    );
    act(() => emitLine?.("old-account-secret /private/user-a"));
    expect(
      screen.getByText("old-account-secret /private/user-a"),
    ).toBeTruthy();

    act(() => emitReset?.());
    expect(
      screen.queryByText("old-account-secret /private/user-a"),
    ).toBeNull();
    expect(screen.getByText("Showing 0 of 0")).toBeTruthy();
  });
});
