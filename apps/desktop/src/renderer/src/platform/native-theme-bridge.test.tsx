// @vitest-environment jsdom
import { cleanup, render } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { NativeThemeBridge } from "./native-theme-bridge";

const state = vi.hoisted(() => ({ theme: undefined as string | undefined }));
vi.mock("@patchbay/ui/components/common/theme-provider", () => ({
  useTheme: () => ({ theme: state.theme }),
}));

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
  state.theme = undefined;
});

function setup() {
  const setNativeTheme = vi.fn();
  vi.stubGlobal("desktopAPI", { setNativeTheme });
  return setNativeTheme;
}

describe("NativeThemeBridge", () => {
  it("updates native materials when the main window changes appearance", () => {
    const setNativeTheme = setup();
    state.theme = "light";
    const { rerender } = render(<NativeThemeBridge enabled />);
    expect(setNativeTheme).toHaveBeenLastCalledWith("light");
    state.theme = "dark";
    rerender(<NativeThemeBridge enabled />);
    expect(setNativeTheme).toHaveBeenLastCalledWith("dark");
    // Forward the preference, not resolvedTheme: system must keep tracking OS changes.
    state.theme = "system";
    rerender(<NativeThemeBridge enabled />);
    expect(setNativeTheme).toHaveBeenLastCalledWith("system");
  });

  it("does not let an issue window override the app appearance", () => {
    const setNativeTheme = setup();
    state.theme = "light";
    render(<NativeThemeBridge enabled={false} />);
    expect(setNativeTheme).not.toHaveBeenCalled();
  });

  it("waits for a valid hydrated preference", () => {
    const setNativeTheme = setup();
    const { rerender } = render(<NativeThemeBridge enabled />);
    state.theme = "invalid";
    rerender(<NativeThemeBridge enabled />);
    expect(setNativeTheme).not.toHaveBeenCalled();
  });
});
