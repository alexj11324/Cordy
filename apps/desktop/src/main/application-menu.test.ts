// @vitest-environment node
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { MenuItemConstructorOptions } from "electron";

const ctx = vi.hoisted(() => ({
  preferredLanguages: ["en-US"] as string[],
  menuItemEnabled: true,
  getMenuItemById: vi.fn(),
  setApplicationMenu: vi.fn(),
  buildFromTemplate: vi.fn((template: MenuItemConstructorOptions[]) => ({
    template,
  })),
}));

vi.mock("electron", () => ({
  app: {
    name: "Patchbay",
    getPreferredSystemLanguages: () => ctx.preferredLanguages,
  },
  Menu: {
    buildFromTemplate: ctx.buildFromTemplate,
    setApplicationMenu: ctx.setApplicationMenu,
    getApplicationMenu: () => ({
      getMenuItemById: ctx.getMenuItemById,
    }),
  },
}));

import {
  CHECK_FOR_UPDATES_MENU_ID,
  buildApplicationMenuTemplate,
  checkForUpdatesMenuLabel,
  installApplicationMenu,
} from "./application-menu";

function darwinAppSubmenu(): MenuItemConstructorOptions[] {
  const template = buildApplicationMenuTemplate(() => undefined, "darwin");
  const appMenu = template[0];
  expect(appMenu?.label).toBe("Patchbay");
  expect(Array.isArray(appMenu?.submenu)).toBe(true);
  return appMenu?.submenu as MenuItemConstructorOptions[];
}

function helpSubmenu(
  platform: NodeJS.Platform,
): MenuItemConstructorOptions[] {
  const template = buildApplicationMenuTemplate(() => undefined, platform);
  const help = template.at(-1);
  expect(help?.role).toBe("help");
  expect(Array.isArray(help?.submenu)).toBe(true);
  return help?.submenu as MenuItemConstructorOptions[];
}

describe("application menu Check for Updates", () => {
  beforeEach(() => {
    ctx.preferredLanguages = ["en-US"];
    ctx.menuItemEnabled = true;
    ctx.getMenuItemById.mockReset();
    ctx.getMenuItemById.mockImplementation(() => ({
      get enabled() {
        return ctx.menuItemEnabled;
      },
      set enabled(value: boolean) {
        ctx.menuItemEnabled = value;
      },
    }));
    ctx.setApplicationMenu.mockReset();
    ctx.buildFromTemplate.mockClear();
  });

  it("places the item under the macOS app name, after About", () => {
    const submenu = darwinAppSubmenu();
    expect(submenu.map((item) => item.role ?? item.id ?? item.type)).toEqual([
      "about",
      "separator",
      CHECK_FOR_UPDATES_MENU_ID,
      "separator",
      "services",
      "separator",
      "hide",
      "hideOthers",
      "unhide",
      "separator",
      "quit",
    ]);
    expect(submenu[2]?.label).toBe("Check for Updates…");
  });

  it("keeps the command on Help outside macOS", () => {
    const submenu = helpSubmenu("linux");
    expect(submenu[0]?.id).toBe(CHECK_FOR_UPDATES_MENU_ID);
    expect(submenu[0]?.label).toBe("Check for Updates…");
    expect(submenu.map((item) => item.role ?? item.id ?? item.type)).toEqual([
      CHECK_FOR_UPDATES_MENU_ID,
      "separator",
      "about",
    ]);
  });

  it("does not duplicate the command on the macOS Help menu", () => {
    const template = buildApplicationMenuTemplate(() => undefined, "darwin");
    const help = template.at(-1);
    expect(help).toEqual({ role: "help" });
  });

  it("localizes the item from the OS language", () => {
    expect(checkForUpdatesMenuLabel(["zh-CN"])).toBe("检查更新…");
    expect(checkForUpdatesMenuLabel(["ja"])).toBe("更新を確認…");
    expect(checkForUpdatesMenuLabel(["ko-KR"])).toBe("업데이트 확인…");
  });

  it("invokes the update check when the item is chosen", async () => {
    const onCheckForUpdates = vi.fn().mockResolvedValue(undefined);
    const submenu = buildApplicationMenuTemplate(
      onCheckForUpdates,
      "darwin",
    )[0]?.submenu as MenuItemConstructorOptions[];
    const item = submenu.find((entry) => entry.id === CHECK_FOR_UPDATES_MENU_ID);
    expect(item?.click).toBeTypeOf("function");

    await item?.click?.(undefined as never, undefined, undefined as never);

    expect(onCheckForUpdates).toHaveBeenCalledTimes(1);
  });

  it("disables the item for the duration of the check", async () => {
    let resolveCheck: (() => void) | undefined;
    const pending = new Promise<void>((resolve) => {
      resolveCheck = resolve;
    });
    const submenu = buildApplicationMenuTemplate(
      () => pending,
      "darwin",
    )[0]?.submenu as MenuItemConstructorOptions[];
    const item = submenu.find((entry) => entry.id === CHECK_FOR_UPDATES_MENU_ID);

    const clickPromise = item?.click?.(
      undefined as never,
      undefined,
      undefined as never,
    );
    expect(ctx.menuItemEnabled).toBe(false);

    resolveCheck?.();
    await clickPromise;
    expect(ctx.menuItemEnabled).toBe(true);
  });

  it("installs the template as the application menu", () => {
    installApplicationMenu(() => undefined);
    expect(ctx.buildFromTemplate).toHaveBeenCalledTimes(1);
    expect(ctx.setApplicationMenu).toHaveBeenCalledTimes(1);
  });
});
