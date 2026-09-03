import { afterEach, describe, expect, it } from "vitest";
import { render, screen } from "@testing-library/react";
import { DragStrip } from "./drag-strip";

afterEach(() => {
  delete (globalThis as typeof globalThis & { desktopAPI?: unknown })
    .desktopAPI;
});

describe("DragStrip", () => {
  it("does not cover controls in a regular browser", () => {
    render(<DragStrip />);

    expect(screen.queryByTestId("native-drag-strip")).not.toBeInTheDocument();
  });

  it("overlays the Electron titlebar without consuming layout height", () => {
    (
      globalThis as typeof globalThis & {
        desktopAPI?: { host: "electron" };
      }
    ).desktopAPI = { host: "electron" };

    render(<DragStrip />);

    const strip = screen.getByTestId("native-drag-strip");
    expect(strip).toHaveClass(
      "fixed",
      "right-0",
      "left-0",
      "top-0",
      "z-50",
      "h-12",
    );
    expect(strip).not.toHaveClass("shrink-0");
  });

  it("leaves a hit-test-safe gap for pinned top-right controls", () => {
    (
      globalThis as typeof globalThis & {
        desktopAPI?: { host: "electron" };
      }
    ).desktopAPI = { host: "electron" };

    render(<DragStrip reserveTrailingControls />);

    expect(screen.getByTestId("native-drag-strip")).toHaveClass("right-40");
  });
});
