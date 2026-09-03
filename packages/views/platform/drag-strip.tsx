import type { CSSProperties } from "react";

/**
 * Native-only overlay for simple full-window surfaces whose top 48px contains
 * no controls. It keeps the window draggable without taking a blank row out
 * of the page layout.
 *
 * Shared views also render on the web, where an invisible fixed element would
 * intercept ordinary browser controls. The preload host marker keeps this
 * overlay out of browser renders. Screens with an interactive top bar own
 * their drag/no-drag regions instead of using this primitive.
 */
export function DragStrip({
  reserveTrailingControls = false,
}: {
  /** Leave room for a host-owned control pinned inside the top-right 48px. */
  reserveTrailingControls?: boolean;
} = {}) {
  const host = (
    globalThis as typeof globalThis & {
      desktopAPI?: { host?: "electron" | "browser" };
    }
  ).desktopAPI?.host;

  if (host !== "electron") return null;

  return (
    <div
      aria-hidden
      data-testid="native-drag-strip"
      className={
        reserveTrailingControls
          ? "fixed top-0 right-40 left-0 z-50 h-12"
          : "fixed top-0 right-0 left-0 z-50 h-12"
      }
      style={{ WebkitAppRegion: "drag" } as CSSProperties}
    />
  );
}
