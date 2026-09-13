import {
  describe,
  it,
  expect,
  beforeAll,
  beforeEach,
  afterAll,
  afterEach,
  vi,
} from "vitest";
import { screen, act, cleanup, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

const mockPersist = vi.hoisted(() => vi.fn());
const mockUpdateMe = vi.hoisted(() => vi.fn());
const mockReload = vi.hoisted(() => vi.fn());
const mockToastWarning = vi.hoisted(() => vi.fn());
const mockToastError = vi.hoisted(() => vi.fn());
const mockToastSuccess = vi.hoisted(() => vi.fn());
const mockSetTheme = vi.hoisted(() => vi.fn());
const mockSetUser = vi.hoisted(() => vi.fn());
const userRef = vi.hoisted(() => ({
  current: null as { id: string; timezone?: string | null } | null,
}));

vi.mock("@orvilo/ui/components/common/theme-provider", () => ({
  useTheme: () => ({ theme: "light", setTheme: mockSetTheme }),
}));

vi.mock("@orvilo/core/i18n/react", async () => {
  const actual =
    await vi.importActual<typeof import("@orvilo/core/i18n/react")>(
      "@orvilo/core/i18n/react",
    );
  return {
    ...actual,
    useLocaleAdapter: () => ({
      persist: mockPersist,
      getUserChoice: () => null,
      getSystemPreferences: () => [],
    }),
  };
});

vi.mock("@orvilo/core/api", () => ({
  api: { updateMe: mockUpdateMe },
}));

vi.mock("sonner", () => ({
  toast: {
    warning: mockToastWarning,
    error: mockToastError,
    success: mockToastSuccess,
  },
}));

vi.mock("@orvilo/core/auth", async () => {
  const actual =
    await vi.importActual<typeof import("@orvilo/core/auth")>(
      "@orvilo/core/auth",
    );
  type AuthState = {
    user: typeof userRef.current;
    setUser: typeof mockSetUser;
  };
  const state = (): AuthState => ({
    user: userRef.current,
    setUser: mockSetUser,
  });
  const useAuthStore = Object.assign(
    (sel?: (s: AuthState) => unknown) =>
      sel ? sel(state()) : state(),
    { getState: state },
  );
  return { ...actual, useAuthStore };
});

import { renderWithI18n } from "../../test/i18n";
import { PreferencesTab } from "./preferences-tab";
import { useCommentComposerStore } from "@orvilo/core/issues/stores";

/**
 * `lobe: true` mounts the theme bridge, which is imported on demand — so the
 * first query in every test has to be an async one. This helper is that first
 * query, so no test has to remember the rule twice.
 *
 * It returns the tab's single group, and every row query below goes through it.
 * Scoping the query to its group skips computing the accessible name of every
 * other candidate in the document, which is where the cost of a named role
 * query sits.
 *
 * Not for disambiguation, and the earlier version of this paragraph was
 * self-contradictory where it tried to say so: it returned "the tab's single
 * group" and then justified the scope by rows appearing "in more than one of
 * these tabs' groups". A single group means this document holds one of each
 * name, so a document-wide query resolves. (The tabs that genuinely do need the
 * scope for ambiguity — `issue-tab`, whose two create modes really do repeat
 * Priority / Project / Due date — say so on their own.)
 */
async function renderTab() {
  renderWithI18n(<PreferencesTab />, { lobe: true });
  // Returned rather than assigned to a module-level binding: the picking
  // helpers below take it as their first argument. A shared mutable binding
  // would outlive the render that created it — `findByRole` resolves against
  // whatever the document holds at the time, so a stale handle from a previous
  // test would keep resolving against the previous test's tree wherever the
  // two overlap. `notifications-tab.test.tsx` returns its groups the same way;
  // this file and that one should not teach two shapes for one job.
  return within(await screen.findByRole("group", { name: "General" }));
}

/**
 * Picks an option out of one of the selects.
 *
 * The click is scoped to the open popup rather than the document, because the
 * trigger carries a `title` equal to the *selected* label — picking the value
 * that is already selected would match the trigger and the option both.
 *
 * The popup is reached through `[role=listbox]`, which antd renders as a
 * clipped mirror of the options for assistive technology, and takes the click
 * on the item next to it: those mirror nodes are not the ones a pointer
 * reaches, and the items that are carry no role at all. The mirror is also
 * windowed to the active index, so it cannot be used to enumerate a long list
 * like the timezone one — hence `getByTitle` for the item and nothing else.
 */
async function pickOption(
  tab: ReturnType<typeof within>,
  user: ReturnType<typeof userEvent.setup>,
  comboboxName: string,
  optionName: string | RegExp,
) {
  await user.click(tab.getByRole("combobox", { name: comboboxName }));
  const mirror = await screen.findByRole("listbox");
  await user.click(
    within(mirror.parentElement as HTMLElement).getByTitle(optionName),
  );
}

describe("PreferencesTab — Language switcher", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    userRef.current = null;
    vi.useFakeTimers({ shouldAdvanceTime: true });
    Object.defineProperty(window, "location", {
      writable: true,
      configurable: true,
      value: { reload: mockReload },
    });
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  async function pickLanguage(
    tab: ReturnType<typeof within>,
    user: ReturnType<typeof userEvent.setup>,
    name: string,
  ) {
    await pickOption(tab, user, "Language", name);
  }

  it("does nothing when clicking the current locale", async () => {
    const user = userEvent.setup({ advanceTimers: vi.advanceTimersByTime });
    const tab = await renderTab();

    await pickLanguage(tab, user, "English");

    expect(mockPersist).not.toHaveBeenCalled();
    expect(mockUpdateMe).not.toHaveBeenCalled();
    expect(mockReload).not.toHaveBeenCalled();
  });

  it("shows a confirmation toast when the theme is saved locally", async () => {
    const user = userEvent.setup({ advanceTimers: vi.advanceTimersByTime });
    const tab = await renderTab();

    await pickOption(tab, user, "Theme", "Dark");

    expect(mockSetTheme).toHaveBeenCalledWith("dark");
    expect(mockToastSuccess).toHaveBeenCalledTimes(1);
  });

  it("when not logged in: persists + reloads, no PATCH", async () => {
    const user = userEvent.setup({ advanceTimers: vi.advanceTimersByTime });
    const tab = await renderTab();

    await pickLanguage(tab, user, "中文");

    expect(mockPersist).toHaveBeenCalledWith("zh-Hans");
    expect(mockUpdateMe).not.toHaveBeenCalled();
    expect(mockToastSuccess).toHaveBeenCalledTimes(1);
    expect(mockReload).not.toHaveBeenCalled();
    act(() => vi.advanceTimersByTime(900));
    expect(mockReload).toHaveBeenCalledTimes(1);
    expect(mockToastWarning).not.toHaveBeenCalled();
  });

  it("offers only the locales the product ships", async () => {
    const user = userEvent.setup({ advanceTimers: vi.advanceTimersByTime });
    const tab = await renderTab();

    await user.click(tab.getByRole("combobox", { name: "Language" }));

    const options = await screen.findAllByRole("option");
    expect(options).toHaveLength(2);
    expect(options[0]).toHaveAccessibleName("English");
    expect(options[1]).toHaveAccessibleName("中文");
  });

  it("when logged in + PATCH success: confirms the save before reloading", async () => {
    userRef.current = { id: "user-1" };
    mockUpdateMe.mockResolvedValueOnce({});
    const user = userEvent.setup({ advanceTimers: vi.advanceTimersByTime });
    const tab = await renderTab();

    await pickLanguage(tab, user, "中文");

    expect(mockPersist).toHaveBeenCalledWith("zh-Hans");
    expect(mockUpdateMe).toHaveBeenCalledWith({ language: "zh-Hans" });
    expect(mockToastWarning).not.toHaveBeenCalled();
    expect(mockToastSuccess).toHaveBeenCalledTimes(1);
    expect(mockReload).not.toHaveBeenCalled();
    act(() => vi.advanceTimersByTime(900));
    expect(mockReload).toHaveBeenCalledTimes(1);
  });

  it("when logged in + PATCH fails: shows toast and delays reload by 2.5s", async () => {
    userRef.current = { id: "user-1" };
    mockUpdateMe.mockRejectedValueOnce(new Error("network"));
    const user = userEvent.setup({ advanceTimers: vi.advanceTimersByTime });
    const tab = await renderTab();

    await pickLanguage(tab, user, "中文");

    // Local persist still happened so the reload below sees the new locale.
    expect(mockPersist).toHaveBeenCalledWith("zh-Hans");
    expect(mockUpdateMe).toHaveBeenCalledWith({ language: "zh-Hans" });
    // Toast surfaced the sync failure.
    expect(mockToastWarning).toHaveBeenCalledTimes(1);
    // Reload deferred so the toast is visible.
    expect(mockReload).not.toHaveBeenCalled();

    act(() => {
      vi.advanceTimersByTime(2500);
    });
    expect(mockReload).toHaveBeenCalledTimes(1);
  });
});

describe("PreferencesTab — Timezone section", () => {
  // Shrink the picker to the curated COMMON_TIMEZONES fallback. With the
  // real Intl.supportedValuesOf the popup renders ~600 options, and
  // userEvent traversal of that list blew past the per-test timeout on
  // slow CI runners (MUL-4427). Everything these tests pick — a zone from
  // the curated list and the "(browser)" sentinel — exists in it too.
  const intlWithValues = Intl as typeof Intl & {
    supportedValuesOf?: (key: "timeZone") => string[];
  };
  const realSupportedValuesOf = intlWithValues.supportedValuesOf;
  beforeAll(() => {
    intlWithValues.supportedValuesOf = () => [];
  });
  afterAll(() => {
    intlWithValues.supportedValuesOf = realSupportedValuesOf;
  });

  beforeEach(() => {
    vi.clearAllMocks();
    userRef.current = null;
  });

  // The select portals its popup onto document.body; unmount each render
  // fully between tests so a prior test's trigger/popup can't shadow the
  // next one's.
  afterEach(() => {
    cleanup();
  });

  // Opens the Select popup and clicks the option whose label matches.
  // Re-queries the trigger each call so it operates on the current render,
  // never a stale node.
  //
  // The zones these tests pick are near the top of the list on purpose: antd
  // renders a virtual window over the popup, and under jsdom only the first
  // screenful (nine of the curator's nineteen) exists in the DOM at all.
  async function pickTimezone(
    tab: ReturnType<typeof within>,
    user: ReturnType<typeof userEvent.setup>,
    name: RegExp | string,
  ) {
    await pickOption(tab, user, "Viewing Timezone", name);
  }

  // The migration's original defect, and the guard against it returning.
  //
  // The type scale has to reach the option *elements*, and it has to be inline
  // style, because antd sets `font-size` on those same elements with a
  // selector that outranks a utility class — a class on the popup *container*
  // did nothing at all. Nothing caught that the first time: the mono face
  // still applied (antd does not contest `font-family` on an option), so only
  // the size was wrong, and 12-vs-14px is not a difference a screenshot
  // adjudicates. jsdom does not resolve `var()`, so this asserts the
  // declaration rather than a resolved pixel — which is exactly the layer the
  // regression happened at.
  it("puts the caption/mono type scale on the trigger and on the options", async () => {
    userRef.current = { id: "user-1", timezone: "Asia/Shanghai" };
    const user = userEvent.setup();
    const tab = await renderTab();

    const combobox = () =>
      tab.getByRole("combobox", { name: "Viewing Timezone" });

    expect(combobox().closest(".ant-select")).toHaveAttribute(
      "style",
      expect.stringContaining("--text-caption"),
    );

    await user.click(combobox());
    const mirror = await screen.findByRole("listbox");
    expect(
      within(mirror.parentElement as HTMLElement).getByTitle("Asia/Shanghai"),
    ).toHaveAttribute("style", expect.stringContaining("--text-caption"));
  });

  // The other half of the same invariant: shrinking the type must not shrink
  // the control. `.ant-select` sets no height — its box is padding + the
  // content's line-height — so the 12px trigger rendered 30px tall beside two
  // 36px siblings in the same card, and the padding is a token antd derived
  // from the default font size, which an inline `font-size` cannot move.
  //
  // **Reach, stated honestly: this pins the declaration, not the box.** jsdom
  // has no layout engine, so it cannot see a rendered height at all. The
  // assertion fails if someone removes the `min-height`; it cannot fail if the
  // token resolves to the wrong number, or if a future antd stops deriving the
  // box this way. The box is verified by reading computed heights in the
  // running renderer, which is the only place this defect was ever visible.
  it("pins the trigger to the select's own height token so the type scale cannot shrink the box", async () => {
    const tab = await renderTab();

    expect(
      tab
        .getByRole("combobox", { name: "Viewing Timezone" })
        .closest(".ant-select"),
    ).toHaveAttribute("style", expect.stringContaining("--ant-select-height"));
  });

  it("renders the stored timezone in the trigger", async () => {
    userRef.current = { id: "user-1", timezone: "Asia/Shanghai" };
    const tab = await renderTab();

    // Asserted through the trigger's `title` rather than the combobox's own
    // text content: this select renders `<input role="combobox">`, whose text
    // content is empty by construction — the selected label is a sibling node,
    // and antd mirrors it into that node's `title`.
    expect(tab.getByTitle("Asia/Shanghai")).toBeInTheDocument();
  });

  // handleChange PATCHes then updates the store asynchronously, so the
  // post-pick assertions must waitFor it to settle.
  it("saving a new timezone PATCHes /api/me and updates the auth store", async () => {
    userRef.current = { id: "user-1", timezone: "Asia/Shanghai" };
    const updatedUser = { id: "user-1", timezone: "America/New_York" };
    mockUpdateMe.mockResolvedValueOnce(updatedUser);
    const user = userEvent.setup();
    const tab = await renderTab();

    await pickTimezone(tab, user, "America/New_York");

    await waitFor(() => {
      expect(mockUpdateMe).toHaveBeenCalledWith({ timezone: "America/New_York" });
      expect(mockSetUser).toHaveBeenCalledWith(updatedUser);
      expect(mockToastSuccess).toHaveBeenCalledTimes(1);
    });
  });

  it("surfaces a toast when the PATCH fails", async () => {
    userRef.current = { id: "user-1", timezone: "Asia/Shanghai" };
    mockUpdateMe.mockRejectedValueOnce(new Error("network down"));
    const user = userEvent.setup();
    const tab = await renderTab();

    await pickTimezone(tab, user, "America/New_York");

    await waitFor(() => {
      expect(mockUpdateMe).toHaveBeenCalledWith({ timezone: "America/New_York" });
      expect(mockToastError).toHaveBeenCalledTimes(1);
    });
    expect(mockSetUser).not.toHaveBeenCalled();
  });

  it("clearing the preference sends an empty-string timezone", async () => {
    userRef.current = { id: "user-1", timezone: "Asia/Shanghai" };
    const clearedUser = { id: "user-1", timezone: null };
    mockUpdateMe.mockResolvedValueOnce(clearedUser);
    const user = userEvent.setup();
    const tab = await renderTab();

    // The "(browser)" sentinel option resets the preference to NULL; the
    // wire payload is an empty string the backend translates to NULL.
    await pickTimezone(tab, user, /browser/i);

    await waitFor(() => {
      expect(mockUpdateMe).toHaveBeenCalledWith({ timezone: "" });
      // The PATCH response (timezone: null) is pushed into the auth store
      // so the picker switches back to "(browser)" without a refetch.
      expect(mockSetUser).toHaveBeenCalledWith(clearedUser);
    });
  });
});

describe("PreferencesTab — Sticky comment bar", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    userRef.current = null;
    useCommentComposerStore.setState({ sticky: true });
  });

  afterEach(() => {
    cleanup();
  });

  it("renders on by default and toggles the preference off with a saved toast", async () => {
    const user = userEvent.setup();
    const tab = await renderTab();

    // Named query, not an index into the page's switches: the switch's
    // accessible name is the only thing that survives a wrapper changing.
    const toggle = tab.getByRole("switch", { name: "Sticky comment bar" });
    expect(toggle).toHaveAttribute("aria-checked", "true");

    await user.click(toggle);

    expect(useCommentComposerStore.getState().sticky).toBe(false);
    expect(toggle).toHaveAttribute("aria-checked", "false");
    expect(mockToastSuccess).toHaveBeenCalledTimes(1);
  });
});
