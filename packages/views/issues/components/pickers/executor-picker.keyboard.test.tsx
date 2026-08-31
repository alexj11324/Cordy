// Keyboard behaviour of the executor picker's search box. Uses the REAL
// PropertyPicker / Base UI Popover — the regression under test lives in
// PropertyPicker's highlight bookkeeping, so mocking it would test nothing.
//
// Regression: the "Unassigned" row is pinned as the picker's first row, which
// also makes it the first `data-picker-item`. PropertyPicker used to reset the
// highlight to index 0 on every keystroke, so typing a name and pressing Enter
// unassigned the issue instead of assigning it to the person being searched
// for. The empty row is now excluded from the search-result Enter default while
// staying reachable by arrow key.
import { describe, expect, it, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { I18nProvider } from "@patchbay/core/i18n/react";
import enIssues from "../../../locales/en/issues.json";
import { ExecutorPicker } from "./executor-picker";

const MEMBERS = [
  { user_id: "user-1", name: "Ada Lovelace", role: "member" },
  { user_id: "user-2", name: "Grace Hopper", role: "member" },
];

vi.mock("@tanstack/react-query", () => ({
  useQuery: ({ queryKey }: { queryKey: string[] }) => {
    if (queryKey[0] === "members") return { data: MEMBERS };
    return { data: [] };
  },
}));

vi.mock("@patchbay/core/hooks", () => ({ useWorkspaceId: () => "workspace-1" }));
vi.mock("@patchbay/core/auth", () => ({ useAuthStore: () => ({ id: "user-1" }) }));
vi.mock("@patchbay/core/agents", () => ({ isAgentRuntimeBound: () => true }));
vi.mock("@patchbay/core/permissions", () => ({
  canAssignAgentToIssue: () => ({ allowed: true }),
}));
vi.mock("@patchbay/core/workspace/hooks", () => ({
  useActorName: () => ({ getActorName: () => "Ada Lovelace" }),
}));
vi.mock("@patchbay/core/workspace/queries", () => ({
  memberListOptions: () => ({ queryKey: ["members"] }),
  agentListOptions: () => ({ queryKey: ["agents"] }),
  teamListOptions: () => ({ queryKey: ["teams"] }),
  executorFrequencyOptions: () => ({ queryKey: ["frequency"] }),
}));
vi.mock("../../../common/actor-avatar", () => ({
  ActorAvatar: () => <span data-testid="actor-avatar" />,
}));

const SEARCH_PLACEHOLDER = "Assign to...";

function renderPicker(onUpdate: () => void) {
  return render(
    <I18nProvider locale="en" resources={{ en: { issues: enIssues } }}>
      {/* Controlled open: the picker is the subject, its trigger is not. */}
      <ExecutorPicker
        executorType={null}
        executorId={null}
        onUpdate={onUpdate}
        open
        onOpenChange={() => {}}
      />
    </I18nProvider>,
  );
}

describe("ExecutorPicker search keyboard defaults", () => {
  it("assigns the first match on Enter instead of unassigning", async () => {
    const user = userEvent.setup();
    const onUpdate = vi.fn();

    renderPicker(onUpdate);

    await user.type(await screen.findByPlaceholderText(SEARCH_PLACEHOLDER), "grace");
    await user.keyboard("{Enter}");

    expect(onUpdate).toHaveBeenCalledWith({
      owner_type: "member", owner_id: "user-2",
    });
  });

  it("leaves Enter inert when the query matches nobody", async () => {
    const user = userEvent.setup();
    const onUpdate = vi.fn();

    renderPicker(onUpdate);

    await user.type(await screen.findByPlaceholderText(SEARCH_PLACEHOLDER), "zzzznomatch");
    await user.keyboard("{Enter}");

    expect(onUpdate).not.toHaveBeenCalled();
  });
});
