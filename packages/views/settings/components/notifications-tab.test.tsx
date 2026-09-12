import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { renderWithI18n } from "../../test/i18n";

const mockMutate = vi.hoisted(() => vi.fn());
const mockToastSuccess = vi.hoisted(() => vi.fn());
const mockToastError = vi.hoisted(() => vi.fn());
const mockRequestPermission = vi.hoisted(() => vi.fn());
const preferencesRef = vi.hoisted(() => ({
  current: {} as Record<string, string>,
}));

vi.mock("@orvilo/core/hooks", () => ({ useWorkspaceId: () => "workspace-1" }));

vi.mock("@tanstack/react-query", () => ({
  useQuery: () => ({ data: { preferences: preferencesRef.current } }),
}));

vi.mock("@orvilo/core/notification-preferences/queries", () => ({
  notificationPreferenceOptions: () => ({
    queryKey: ["notification-preferences", "workspace-1"],
    queryFn: vi.fn(),
  }),
}));

vi.mock("@orvilo/core/notification-preferences/mutations", () => ({
  useUpdateNotificationPreferences: () => ({ mutate: mockMutate }),
}));

vi.mock("sonner", () => ({
  toast: { success: mockToastSuccess, error: mockToastError },
}));

vi.mock("@orvilo/core/platform", () => ({
  getWebNotificationPermission: () => "default",
  isWebNotificationSupported: () => true,
  requestWebNotificationPermission: mockRequestPermission,
}));

vi.mock("../../platform", () => ({ isDesktopShell: () => false }));

import { NotificationsTab } from "./notifications-tab";

// The whole suite mounts Lobe form rows and switches, three times over. Under
// full-suite parallelism each mount is seconds rather than milliseconds (the
// same reason `vitest.config.ts` raises the global budget for antd), so the
// hooks that unmount them get their own budget too.
vi.setConfig({ testTimeout: 60_000, hookTimeout: 30_000 });

/**
 * `lobe: true` loads the theme bridge on demand, so the first query is async.
 * The two groups are returned by name; every switch query goes through them,
 * because a document-wide named role query costs ~3.4s against a scoped one's
 * ~9ms (measured on this component family) and because the group is the handle
 * the reference tells every tab to use.
 */
async function renderTab() {
  renderWithI18n(<NotificationsTab />, { lobe: true });
  const inbox = within(
    await screen.findByRole("group", { name: "Inbox Notifications" }),
  );
  const system = within(
    await screen.findByRole("group", { name: "System Notifications" }),
  );
  return { inbox, system };
}

describe("NotificationsTab", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    preferencesRef.current = {};
  });

  afterEach(() => {
    cleanup();
  });

  it("reads each switch's state from the stored preferences", async () => {
    preferencesRef.current = { mentions: "muted", system_notifications: "muted" };
    const { inbox, system } = await renderTab();

    expect(inbox.getByRole("switch", { name: "Assignments" })).toBeChecked();
    expect(inbox.getByRole("switch", { name: "Mentions" })).not.toBeChecked();
    // `system_notifications` is the sibling key rendered in its own group.
    expect(
      system.getByRole("switch", { name: "Show system notifications" }),
    ).not.toBeChecked();
  });

  it("mutes an enabled group by patching the whole preference object", async () => {
    const user = userEvent.setup();
    const { inbox } = await renderTab();

    await user.click(inbox.getByRole("switch", { name: "Comments" }));

    expect(mockMutate).toHaveBeenCalledTimes(1);
    expect(mockMutate.mock.calls[0]![0]).toEqual({ comments: "muted" });
  });

  it("un-muting drops the key instead of writing \"all\"", async () => {
    preferencesRef.current = { comments: "muted" };
    const user = userEvent.setup();
    const { inbox } = await renderTab();

    await user.click(inbox.getByRole("switch", { name: "Comments" }));

    // "all" is the default, so the object is kept clean by removing the key.
    expect(mockMutate.mock.calls[0]![0]).toEqual({});
  });

  // Web-only row: it exists to ask for the browser permission the native
  // banners need, and reports the answer back in place.
  it("asks for the browser permission and reports the result", async () => {
    mockRequestPermission.mockResolvedValueOnce("granted");
    const user = userEvent.setup();
    const { system } = await renderTab();

    const enable = system.getByRole("button", { name: "Enable" });
    await user.click(enable);

    expect(mockRequestPermission).toHaveBeenCalledTimes(1);
    // Async query rather than `waitFor` + `getBy`: the assertion is waiting on
    // a state update, and the bridge file's `asyncUtilTimeout` is the budget
    // that already accounts for a loaded machine.
    expect(await screen.findByText("Enabled")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Enable" })).toBeNull();
  });
});
