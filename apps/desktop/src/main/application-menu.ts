import { Menu, app, type MenuItemConstructorOptions } from "electron";
import { preferredAppLocaleFromLanguages } from "./os-locale";

export const CHECK_FOR_UPDATES_MENU_ID = "check-for-updates";

const checkForUpdatesLabel: Record<
  ReturnType<typeof preferredAppLocaleFromLanguages>,
  string
> = {
  en: "Check for Updates…",
  "zh-Hans": "检查更新…",
  ja: "更新を確認…",
  ko: "업데이트 확인…",
};

export function checkForUpdatesMenuLabel(
  languages: readonly string[] = app.getPreferredSystemLanguages(),
): string {
  return checkForUpdatesLabel[preferredAppLocaleFromLanguages(languages)];
}

/**
 * Application menu with a native "Check for Updates…" item.
 *
 * Electron's default `appMenu` has About / Services / Hide / Quit and no
 * update entry. Expanding the macOS app menu (instead of `role: "appMenu"`)
 * is what lets us put the item under the app name, which is where macOS
 * users look for it. Other platforms keep the same command on Help.
 */
export function buildApplicationMenuTemplate(
  onCheckForUpdates: () => void | Promise<void>,
  platform: NodeJS.Platform = process.platform,
  languages: readonly string[] = app.getPreferredSystemLanguages(),
): MenuItemConstructorOptions[] {
  const checkItem: MenuItemConstructorOptions = {
    id: CHECK_FOR_UPDATES_MENU_ID,
    label: checkForUpdatesMenuLabel(languages),
    click: async () => {
      const item = Menu.getApplicationMenu()?.getMenuItemById(
        CHECK_FOR_UPDATES_MENU_ID,
      );
      if (item) item.enabled = false;
      try {
        await onCheckForUpdates();
      } finally {
        if (item) item.enabled = true;
      }
    },
  };

  const template: MenuItemConstructorOptions[] = [];

  if (platform === "darwin") {
    template.push({
      label: app.name,
      submenu: [
        { role: "about" },
        { type: "separator" },
        checkItem,
        { type: "separator" },
        { role: "services" },
        { type: "separator" },
        { role: "hide" },
        { role: "hideOthers" },
        { role: "unhide" },
        { type: "separator" },
        { role: "quit" },
      ],
    });
  }

  template.push({ role: "fileMenu" });
  template.push({ role: "editMenu" });
  template.push({ role: "viewMenu" });
  template.push({ role: "windowMenu" });

  if (platform === "darwin") {
    template.push({ role: "help" });
  } else {
    template.push({
      role: "help",
      submenu: [checkItem, { type: "separator" }, { role: "about" }],
    });
  }

  return template;
}

export function installApplicationMenu(
  onCheckForUpdates: () => void | Promise<void>,
): void {
  Menu.setApplicationMenu(
    Menu.buildFromTemplate(buildApplicationMenuTemplate(onCheckForUpdates)),
  );
}
