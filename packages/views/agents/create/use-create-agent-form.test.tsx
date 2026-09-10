// @vitest-environment jsdom

import type { ReactNode } from "react";
import { act, cleanup, renderHook, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { afterEach, describe, expect, it, vi } from "vitest";
import {
  AGENT_CONVERSATION_STARTER_LABEL_MAX_LENGTH,
  AGENT_CONVERSATION_STARTER_MAX_LENGTH,
  buildCreateAgentRequest,
  EMPTY_AGENT_DRAFT,
  toStoredAgentDraft,
  useManualAgentDraftStore,
} from "@orvilo/core/agents";
import { runtimeKeys } from "@orvilo/core/runtimes";
import { workspaceKeys } from "@orvilo/core/workspace/queries";
import { useCreateAgentForm } from "./use-create-agent-form";
import { useManualDraftSync } from "./use-manual-draft-sync";

vi.mock("@orvilo/core/hooks", () => ({ useWorkspaceId: () => "ws-test" }));
vi.mock("@orvilo/core/auth", () => ({
  useAuthStore: (selector: (state: { user: { id: string } }) => unknown) =>
    selector({ user: { id: "user-1" } }),
}));

const validStarter = { label: "Review", prompt: "Review the latest changes" };
const invalidStarters = [
  { label: " ", prompt: "Missing label" },
  { label: "Missing prompt", prompt: " " },
  { label: "x".repeat(AGENT_CONVERSATION_STARTER_LABEL_MAX_LENGTH + 1), prompt: "Too long" },
  { label: "Too long", prompt: "x".repeat(AGENT_CONVERSATION_STARTER_MAX_LENGTH + 1) },
];

function wrapper() {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false, staleTime: Infinity } } });
  client.setQueryData(runtimeKeys.list("ws-test"), [{
    id: "runtime-1", owner_id: "user-1", status: "online", visibility: "private",
  }]);
  client.setQueryData(workspaceKeys.members("ws-test"), []);
  client.setQueryData(workspaceKeys.skills("ws-test"), []);
  return function TestWrapper({ children }: { children: ReactNode }) {
    return <QueryClientProvider client={client}>{children}</QueryClientProvider>;
  };
}

afterEach(() => {
  cleanup();
  useManualAgentDraftStore.getState().clearDraft();
});

describe("agent creation starter validation", () => {
  it("restores and persists only valid starters for a form without the editor", async () => {
    useManualAgentDraftStore.getState().setDraft({ byOwner: { blank: {
      runtimeId: "runtime-1",
      draft: toStoredAgentDraft({
        ...EMPTY_AGENT_DRAFT,
        name: "Restored agent",
        conversationStarters: [validStarter, ...invalidStarters],
      }, null),
    } } });
    const { result, rerender } = renderHook(() => {
      const form = useCreateAgentForm({ omitInvalidConversationStarters: true });
      useManualDraftSync({ duplicateId: null, draft: form.draft, setDraft: form.setDraft, ready: true });
      return form;
    }, { wrapper: wrapper() });

    await waitFor(() => expect(result.current.draft.name).toBe("Restored agent"));
    expect(result.current.draft.conversationStarters).toEqual([validStarter]);
    expect(result.current.draftReady).toBe(true);
    expect(buildCreateAgentRequest({ draft: result.current.draft, runtimeId: "runtime-1" }).conversation_starters).toEqual([validStarter]);
    expect(useManualAgentDraftStore.getState().draft.byOwner.blank?.draft.conversation_starters).toEqual([validStarter]);
    const normalizedDraft = result.current.draft;
    rerender();
    expect(result.current.draft).toBe(normalizedDraft);
  });

  it("omits the starter payload when all restored entries are invalid", () => {
    const { result } = renderHook(
      () => useCreateAgentForm({ omitInvalidConversationStarters: true }),
      { wrapper: wrapper() },
    );
    act(() => result.current.setDraft({
      ...EMPTY_AGENT_DRAFT,
      name: "Valid agent",
      runtimeId: "runtime-1",
      conversationStarters: invalidStarters,
    }));
    expect(result.current.draftReady).toBe(true);
    expect(result.current.draft.conversationStarters).toEqual([]);
    expect(buildCreateAgentRequest({ draft: result.current.draft, runtimeId: "runtime-1" })).not.toHaveProperty("conversation_starters");
  });

  it("keeps invalid entries editable and blocks creation by default", () => {
    const { result } = renderHook(() => useCreateAgentForm(), { wrapper: wrapper() });
    act(() => result.current.setDraft({
      ...EMPTY_AGENT_DRAFT,
      name: "Builder agent",
      runtimeId: "runtime-1",
      conversationStarters: [validStarter, ...invalidStarters],
    }));
    expect(result.current.draftReady).toBe(false);
    expect(result.current.draft.conversationStarters).toEqual([validStarter, ...invalidStarters]);
    act(() => result.current.setDraft((current) => ({ ...current, conversationStarters: [validStarter] })));
    expect(result.current.draftReady).toBe(true);
  });
});
