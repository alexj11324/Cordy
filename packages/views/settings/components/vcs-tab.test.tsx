// @vitest-environment jsdom

/**
 * `vcs-tab.tsx` had **no test file at all** before Task 8b, which is the whole
 * reason this one exists: the tab's two migration traps are both silent, and
 * the brief names them.
 *
 * 1. Its three connect-form fields were `<Label htmlFor>` + a control carrying
 *    the matching `id`. The mapping deletes `Label`, and deleting it without
 *    moving the association leaves all three controls unnamed — no crash, no
 *    warning, and nothing in the suite to notice. The first case below asserts
 *    the accessible names, which is the only thing that can see it.
 * 2. Its row buttons were `<Button variant="outline" size="sm">`. Lobe's
 *    `Button` has no `variant` and its `size` union has no `"sm"`, and
 *    `Button.mjs` spreads `...rest` onto the DOM — so `variant` reaches the DOM
 *    as an invalid attribute with no error anywhere. The second case asserts it
 *    did not.
 *
 * `{ lobe: true }` is required: the tab is Lobe now, and `renderWithI18n`
 * defaults `lobe` to `false`.
 */

import { describe, it, expect, beforeEach, vi } from "vitest";
import "@testing-library/jest-dom/vitest";
import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { renderWithI18n } from "../../test/i18n";

const mockUpdateWorkspace = vi.hoisted(() => vi.fn());
const mockConnectVCS = vi.hoisted(() => vi.fn());
const mockRotateVCSWebhook = vi.hoisted(() => vi.fn());
const mockDeleteVCSConnection = vi.hoisted(() => vi.fn());
const mockInvalidate = vi.hoisted(() => vi.fn());

const connectionsRef = vi.hoisted(() => ({
  current: {
    configured: true,
    can_manage: true,
    connections: [
      {
        id: "conn-1",
        provider: "forgejo",
        instance_url: "https://forgejo.example.com",
        account_login: "acme-bot",
      },
    ],
  } as {
    configured: boolean;
    can_manage: boolean;
    connections: {
      id: string;
      provider: string;
      instance_url: string;
      account_login: string;
    }[];
  },
}));

vi.mock("@tanstack/react-query", () => ({
  useQuery: () => ({ data: connectionsRef.current }),
  useQueryClient: () => ({ invalidateQueries: mockInvalidate }),
  queryOptions: <T,>(options: T) => options,
}));

vi.mock("@orvilo/core/hooks", () => ({
  useWorkspaceId: () => "workspace-1",
}));

vi.mock("@orvilo/core/vcs", () => ({
  vcsConnectionsOptions: () => ({ queryKey: ["vcs", "workspace-1"] }),
}));

vi.mock("@orvilo/core/api", () => ({
  api: {
    updateWorkspace: mockUpdateWorkspace,
    connectVCS: mockConnectVCS,
    rotateVCSWebhook: mockRotateVCSWebhook,
    deleteVCSConnection: mockDeleteVCSConnection,
  },
}));

vi.mock("sonner", () => ({
  toast: { success: vi.fn(), error: vi.fn() },
}));

import { VCSTab } from "./vcs-tab";

async function renderTab() {
  const result = renderWithI18n(<VCSTab />, { lobe: true });
  // The page description renders unconditionally, so it is the handle to wait
  // on for the lazy bridge to resolve.
  await screen.findByText(/Connect a self-hosted Forgejo/);
  return result;
}

describe("VCSTab", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    connectionsRef.current = {
      configured: true,
      can_manage: true,
      connections: [
        {
          id: "conn-1",
          provider: "forgejo",
          instance_url: "https://forgejo.example.com",
          account_login: "acme-bot",
        },
      ],
    };
  });

  // The `Label` → `SettingsFormRow htmlFor` move, and the only assertion that
  // can see it: without the association each of these is an unnamed control.
  it("names the three connect-form controls", async () => {
    await renderTab();

    expect(screen.getByRole("combobox", { name: "Provider" })).toBeInTheDocument();
    expect(screen.getByRole("textbox", { name: "Instance URL" })).toBeInTheDocument();
    const token = document.getElementById("vcs-token");
    expect(token).not.toBeNull();
    // `type="password"` has no role, so the association is asserted through the
    // label that points at it rather than through an accessible query.
    expect(document.querySelector('label[for="vcs-token"]')).not.toBeNull();
    expect(token?.getAttribute("placeholder")).toBe("Access token");
  });

  // `variant` is not in Lobe's `ButtonProps`, and `Button.mjs` spreads `...rest`
  // onto the DOM — so a leftover `variant="outline"` would land as an invalid
  // attribute rather than a type error.
  it("carries no stray shadcn attributes on the row buttons", async () => {
    await renderTab();

    const regenerate = screen.getByRole("button", { name: "Regenerate webhook" });
    const disconnect = screen.getByRole("button", { name: "Disconnect" });
    for (const button of [regenerate, disconnect]) {
      expect(button.hasAttribute("variant")).toBe(false);
      expect(button.getAttribute("size")).toBeNull();
    }
  });

  // The caller half of `confirmModal`'s contract: `handleRotateWebhook` rethrows,
  // so a failed rotate leaves the dialog open instead of closing on a secret
  // that was never regenerated.
  it("keeps the rotate confirmation open when the request fails", async () => {
    mockRotateVCSWebhook.mockRejectedValue(new Error("boom"));
    const user = userEvent.setup();
    await renderTab();

    await user.click(screen.getByRole("button", { name: "Regenerate webhook" }));
    expect(screen.getByText(/The current webhook secret will stop working/)).toBeTruthy();
    expect(mockRotateVCSWebhook).not.toHaveBeenCalled();

    const dialog = await screen.findByRole("dialog");
    await user.click(within(dialog).getByRole("button", { name: "Regenerate" }));

    await waitFor(() => expect(mockRotateVCSWebhook).toHaveBeenCalled());
    await new Promise((resolve) => setTimeout(resolve, 1_200));
    expect(screen.getByRole("dialog")).toBeInTheDocument();
  });
});
