import type { BrowserWindowConstructorOptions } from "electron";

type MainWindowAppearance = Pick<
  BrowserWindowConstructorOptions,
  "transparent" | "backgroundColor" | "vibrancy" | "visualEffectState"
>;

/**
 * Native macOS sidebar material for the main shell. Renderer surfaces decide
 * which regions remain opaque; issue windows and non-macOS platforms keep the
 * normal solid BrowserWindow treatment.
 */
export function resolveMainWindowAppearance(
  platform: NodeJS.Platform,
): Partial<MainWindowAppearance> {
  if (platform !== "darwin") return {};

  return {
    transparent: true,
    backgroundColor: "#00000000",
    vibrancy: "sidebar",
    visualEffectState: "active",
  };
}
