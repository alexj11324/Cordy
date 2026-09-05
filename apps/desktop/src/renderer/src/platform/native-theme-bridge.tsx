import { useEffect } from "react";
import { useTheme } from "@patchbay/ui/components/common/theme-provider";

/** Only the main window may choose the application-wide native appearance. */
export function NativeThemeBridge({ enabled }: { enabled: boolean }) {
  const { theme } = useTheme();
  useEffect(() => {
    if (enabled && (theme === "light" || theme === "dark" || theme === "system")) {
      window.desktopAPI.setNativeTheme(theme);
    }
  }, [enabled, theme]);
  return null;
}
