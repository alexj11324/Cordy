import { render } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

const runtimesPage = vi.fn<(props: Record<string, unknown>) => null>(() => null);
const useDesktopRuntimeContext = vi.fn();
const isDesktopWebPreview = vi.hoisted(() => vi.fn(() => false));

vi.mock("@patchbay/views/runtimes", () => ({
  RuntimesPage: (props: Record<string, unknown>) => runtimesPage(props),
}));

vi.mock("./use-desktop-runtime-context", () => ({
  useDesktopRuntimeContext: () => useDesktopRuntimeContext(),
}));

vi.mock("../platform/web-bridge", () => ({
  isDesktopWebPreview,
}));

import { DesktopRuntimesPage } from "./desktop-runtimes-page";

describe("DesktopRuntimesPage", () => {
  beforeEach(() => {
    runtimesPage.mockClear();
    useDesktopRuntimeContext.mockReturnValue({
      localDaemonId: "daemon-local",
      localMachineName: "Jiayuan's MacBook",
      bootstrapping: false,
    });
    isDesktopWebPreview.mockReturnValue(false);
  });

  it("keeps daemon controls out of the machine collection", () => {
    render(<DesktopRuntimesPage />);

    expect(runtimesPage).toHaveBeenCalledWith({
      readOnly: false,
      localDaemonId: "daemon-local",
      localMachineName: "Jiayuan's MacBook",
      hasLocalMachine: true,
      bootstrapping: false,
    });
  });

  it("marks the browser preview read-only and omits the synthetic local machine", () => {
    isDesktopWebPreview.mockReturnValue(true);

    render(<DesktopRuntimesPage />);

    expect(runtimesPage).toHaveBeenCalledWith({
      readOnly: true,
      localDaemonId: "daemon-local",
      localMachineName: "Jiayuan's MacBook",
      hasLocalMachine: false,
      bootstrapping: false,
    });
  });
});
