import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  cleanup,
  fireEvent,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
import {
  createShortcutChord,
  configureShortcutPlatform,
  getShortcut,
  useShortcutStore,
} from "@orvilo/core/shortcuts";
import { renderWithI18n } from "../../test/i18n";
import { KeyboardShortcutsTab } from "./keyboard-shortcuts-tab";

// This file still needs more than the repo's 20s default, but far less than it
// first did, and the numbers moved twice as the fixes landed. Whole file:
// 3.55s before the tab was migrated, 125-155s with the migrated markup, 45.5s
// once `Empty` stopped coming from the `@lobehub/ui` root barrel (its module
// graph was ~9.5s of the import phase and its components' styles ran on every
// mount) and once every role query was scoped to its group. Per test: 3.8-6.9s
// isolated. 45s is ~6.5x the worst observed, which is runner-load headroom
// rather than a number that hides the next slow suite.
//
// `hookTimeout` moves with it because unmounting those rows again in
// `afterEach` costs the same order of magnitude and has its own budget — under
// full-suite parallelism it was the hook that first hit the ceiling.
vi.setConfig({ testTimeout: 45_000, hookTimeout: 45_000 });

/**
 * `lobe: true` loads the theme bridge on demand, so the first query has to be
 * async.
 */
async function renderTab() {
  renderWithI18n(<KeyboardShortcutsTab />, { lobe: true });
  await screen.findByText("Open search");
}

/**
 * A query bound to one group of the tab, by its name.
 *
 * Every role query in this suite goes through here rather than through
 * `screen`, and that is not style: `getByRole(role, { name })` computes the
 * accessible name of every candidate in the document, and against the antd
 * stylesheet the theme bridge injects that costs **3.4s** per call under jsdom
 * where the same query scoped to its group costs 9ms — measured on this
 * component, not guessed. An un-named `getAllByRole` is cheap (~70ms), so the
 * cost is the name computation, not the role walk.
 *
 * The group carries `role="group"` with its title as the accessible name
 * (`SettingsGroup` adds the wrapper rc-collapse cannot: its root goes through
 * `pickAttrs`, which drops `role`). That is both the a11y handle and the cheap
 * query.
 */
async function group(title: string) {
  return within(await screen.findByRole("group", { name: title }));
}

/**
 * The tab's leading toolbar — the group that holds the search box and the
 * page-level "Restore defaults". It has no title on purpose (the dialog header
 * already names the tab), so it has no group role to query and is reached
 * through the search box instead. This is the one place in the suite still
 * anchored on antd's collapse item, and only because there is no name here.
 */
function toolbar() {
  const item = screen.getByRole("searchbox").closest(".ant-collapse-item");
  if (!item) throw new Error("no toolbar container");
  return within(item as HTMLElement);
}

describe("KeyboardShortcutsTab", () => {
  beforeEach(() => {
    configureShortcutPlatform("windows");
    useShortcutStore.getState().resetAll();
  });

  afterEach(() => {
    cleanup();
    configureShortcutPlatform(null);
    useShortcutStore.getState().resetAll();
  });

  it("shows distinct left and right sidebar actions", async () => {
    await renderTab();

    expect(
      (await group("General")).getByRole("button", {
        name: "Change shortcut for Toggle left sidebar",
      }),
    ).toBeInTheDocument();
    const rightSidebarRecorder = (await group("General")).getByRole("button", {
      name: "Change shortcut for Toggle right sidebar",
    });
    expect(within(rightSidebarRecorder).getByTitle("Ctrl")).toHaveTextContent(
      "Ctrl",
    );
    expect(within(rightSidebarRecorder).getByTitle("/")).toHaveTextContent("/");
  });

  it("records a shortcut and applies it immediately", async () => {
    await renderTab();
    const recorder = (await group("General")).getByRole("button", {
      name: "Change shortcut for Open search",
    });

    fireEvent.click(recorder);
    fireEvent.keyDown(recorder, { key: "e", ctrlKey: true });

    expect(getShortcut("openSearch")).toEqual(
      createShortcutChord("E", { primary: true }),
    );
    expect(within(recorder).getByTitle("Ctrl")).toHaveTextContent("Ctrl");
    expect(within(recorder).getByTitle("E")).toHaveTextContent("E");
  });

  it("only captures keys while the recorder is active", async () => {
    await renderTab();
    const recorder = (await group("General")).getByRole("button", {
      name: "Change shortcut for Open search",
    });

    recorder.focus();
    fireEvent.keyDown(recorder, { key: "e", ctrlKey: true });
    expect(getShortcut("openSearch")).toEqual(
      createShortcutChord("K", { primary: true }),
    );

    fireEvent.click(recorder);
    fireEvent.keyDown(recorder, { key: "e", ctrlKey: true });
    expect(getShortcut("openSearch")).toEqual(
      createShortcutChord("E", { primary: true }),
    );

    // Focus remains on the button after a successful recording. Tab must move
    // focus normally instead of silently replacing the shortcut with Tab.
    fireEvent.keyDown(recorder, { key: "Tab" });
    expect(getShortcut("openSearch")).toEqual(
      createShortcutChord("E", { primary: true }),
    );
  });

  it("rejects unsafe plain keys in editors while allowing Send = Enter", async () => {
    await renderTab();
    const searchRecorder = (await group("General")).getByRole("button", {
      name: "Change shortcut for Open search",
    });
    fireEvent.click(searchRecorder);
    fireEvent.keyDown(searchRecorder, { key: "j" });
    expect(getShortcut("openSearch")).toEqual(
      createShortcutChord("K", { primary: true }),
    );
    expect(screen.getByRole("alert")).toHaveTextContent(
      "This key would interfere with typing or basic keyboard navigation.",
    );

    const sendRecorder = (await group("General")).getByRole("button", {
      name: "Change shortcut for Send",
    });
    fireEvent.click(sendRecorder);
    fireEvent.keyDown(sendRecorder, { key: "Enter" });
    expect(getShortcut("send")).toEqual(createShortcutChord("Enter"));
  });

  it("only allows Enter or Primary+Enter for Send", async () => {
    await renderTab();
    const sendRecorder = (await group("General")).getByRole("button", {
      name: "Change shortcut for Send",
    });

    fireEvent.click(sendRecorder);
    fireEvent.keyDown(sendRecorder, { key: "Enter", shiftKey: true });
    expect(getShortcut("send")).toEqual(
      createShortcutChord("Enter", { primary: true }),
    );
    expect(screen.getByRole("alert")).toHaveTextContent(
      "Send can only use Enter or Mod+Enter.",
    );

    fireEvent.keyDown(sendRecorder, { key: "Enter", ctrlKey: true });
    expect(getShortcut("send")).toEqual(
      createShortcutChord("Enter", { primary: true }),
    );
  });

  it("rejects shortcuts already assigned to another action", async () => {
    await renderTab();
    const recorder = (await group("General")).getByRole("button", {
      name: "Change shortcut for Create issue",
    });

    fireEvent.click(recorder);
    fireEvent.keyDown(recorder, { key: "k", ctrlKey: true });

    expect(getShortcut("createIssue")).toEqual(
      createShortcutChord("N", { primary: true }),
    );
    expect(screen.getByRole("alert")).toHaveTextContent(
      "Already used by Open search.",
    );
  });

  it("can disable and restore an action", async () => {
    await renderTab();

    fireEvent.click(
      (await group("General")).getByRole("button", {
        name: "Disable Create issue shortcut",
      }),
    );
    expect(getShortcut("createIssue")).toBeNull();

    fireEvent.click(
      (await group("General")).getByRole("button", { name: "Reset Create issue" }),
    );
    expect(getShortcut("createIssue")).toEqual(
      createShortcutChord("N", { primary: true }),
    );
  });

  // The tab's page-level action used to be passed to `SettingsTab` as
  // `action`, which the settings dialog never rendered — `SettingsTab` returns
  // early inside `SettingsDialogBody` and that branch drops the header
  // entirely. It now rides on the first `Form.Group`'s `extra`, so this suite
  // is also the regression guard for "Restore defaults" existing at all.
  it("confirms before restoring all shortcut defaults", async () => {
    useShortcutStore.getState().setShortcut(
      "openSearch",
      createShortcutChord("J", { primary: true }),
    );
    await renderTab();

    fireEvent.click(
      toolbar().getByRole("button", { name: "Restore defaults" }),
    );
    const dialog = await screen.findByRole("dialog");
    expect(dialog).toHaveTextContent(
      "This will remove every custom shortcut and restore the defaults on this device.",
    );
    expect(getShortcut("openSearch")).toEqual(
      createShortcutChord("J", { primary: true }),
    );

    fireEvent.click(within(dialog).getByRole("button", { name: "Cancel" }));
    await waitFor(() =>
      expect(screen.queryByRole("dialog")).not.toBeInTheDocument(),
    );
    expect(getShortcut("openSearch")).toEqual(
      createShortcutChord("J", { primary: true }),
    );

    fireEvent.click(
      toolbar().getByRole("button", { name: "Restore defaults" }),
    );
    fireEvent.click(
      await screen.findByRole("button", { name: "Restore all defaults" }),
    );
    // The confirm dialog closes only once its promise resolves, so the
    // dismissal is a tick later than the click.
    await waitFor(() =>
      expect(screen.queryByRole("dialog")).not.toBeInTheDocument(),
    );
    expect(getShortcut("openSearch")).toEqual(
      createShortcutChord("K", { primary: true }),
    );
  });
});
