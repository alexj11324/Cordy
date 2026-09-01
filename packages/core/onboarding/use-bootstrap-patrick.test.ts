import { beforeEach, describe, expect, it, vi } from "vitest";
import type { ChatSession, PatrickBootstrapResponse } from "../types";

const mocks = vi.hoisted(() => ({
  createPatrickAgent: vi.fn(),
  createAgent: vi.fn(),
  updateAgent: vi.fn(),
  listChatSessions: vi.fn(),
  createChatSession: vi.fn(),
  startPatrickOnboarding: vi.fn(),
}));

vi.mock("../api", () => ({ api: mocks }));

import { bootstrapPatrick } from "./use-bootstrap-patrick";

const agent = {
  id: "agent-1",
  workspace_id: "ws-1",
  runtime_id: "runtime-1",
  name: "Patrick",
  description: "Your workspace Chief of Staff.",
  instructions: "",
  system_key: "patrick",
  avatar_url: null,
  visibility: "workspace",
  permission_mode: "public_to",
  invocation_targets: [{ target_type: "workspace" }],
  max_concurrent_tasks: 3,
  archived_at: null,
} as PatrickBootstrapResponse;

const session = {
  id: "session-1",
  workspace_id: "ws-1",
  agent_id: "agent-1",
  title: "Getting started with Patrick",
  status: "active",
} as ChatSession;

const input = {
  workspaceSlug: "venus",
  runtimeId: "runtime-1",
  title: "Getting started with Patrick",
  language: "en",
} as const;

beforeEach(() => {
  vi.clearAllMocks();
  mocks.createPatrickAgent.mockResolvedValue({
    ...agent,
    onboarding_session: session,
  });
  mocks.startPatrickOnboarding.mockResolvedValue({
    started: true,
    message_id: "message-1",
    created_at: "2026-01-01T00:00:00Z",
  });
});

describe("bootstrapPatrick", () => {
  it("asks the server for Patrick, then opens one session and one hidden kickoff", async () => {
    const result = await bootstrapPatrick(input);

    // Only a runtime, a language and the session label cross the wire: name,
    // description, avatar, permissions and the system prompt are the server's
    // to decide, and so is which session this member already has.
    expect(mocks.createPatrickAgent).toHaveBeenCalledOnce();
    expect(mocks.createPatrickAgent).toHaveBeenCalledWith(
      {
        runtime_id: "runtime-1",
        language: "en",
        session_title: "Getting started with Patrick",
      },
      "venus",
    );
    expect(mocks.startPatrickOnboarding).toHaveBeenCalledWith(
      "session-1",
      { language: "en" },
      "venus",
    );
    expect(result.agent.id).toBe("agent-1");
    expect(result.chatSession).toBe(session);
  });

  it("never builds an agent payload of its own", async () => {
    await bootstrapPatrick(input);

    // The generic create/update endpoints cannot set system_key, so routing
    // Patrick through them would produce an agent the claim path ignores.
    expect(mocks.createAgent).not.toHaveBeenCalled();
    expect(mocks.updateAgent).not.toHaveBeenCalled();
  });

  it("sends nothing about the member's other workspaces", async () => {
    await bootstrapPatrick(input);

    // Every workspace onboards from scratch. A member's second workspace may
    // carry entirely different work and collaborators, so its opening is built
    // from this workspace's own inputs and nothing else.
    const [, payload] = mocks.startPatrickOnboarding.mock.calls[0]!;
    expect(Object.keys(payload)).toEqual(["language"]);
  });

  // The session is now resolved server-side under an advisory lock. Listing
  // sessions and creating one on a miss was a check-then-insert with nothing
  // serializing it, so two tabs each opened their own onboarding conversation
  // — and because the match was on the localized title, changing language
  // between a failed attempt and its retry opened another.
  it("never resolves the session client-side", async () => {
    mocks.startPatrickOnboarding.mockResolvedValue({ started: false });

    await bootstrapPatrick(input);

    expect(mocks.listChatSessions).not.toHaveBeenCalled();
    expect(mocks.createChatSession).not.toHaveBeenCalled();
    expect(mocks.startPatrickOnboarding).toHaveBeenCalledOnce();
  });

  // Rather than falling back to creating one, which is the race this removed.
  it("fails loudly when the server returns no session", async () => {
    mocks.createPatrickAgent.mockResolvedValue(agent);

    await expect(bootstrapPatrick(input)).rejects.toThrow(/session/i);
    expect(mocks.createChatSession).not.toHaveBeenCalled();
    expect(mocks.startPatrickOnboarding).not.toHaveBeenCalled();
  });

  // PB-5501: on desktop the tab system also writes the global
  // current-workspace singleton, and it reclaims it while this flow runs —
  // the new workspace has no tab group yet. Every request here therefore has
  // to name its own workspace; when they did not, Patrick was provisioned into
  // whichever workspace the shell had switched back to.
  it("names the target workspace on every request, not just the first", async () => {
    await bootstrapPatrick({ ...input, workspaceSlug: "proxima-centauri" });

    for (const call of [mocks.createPatrickAgent, mocks.startPatrickOnboarding]) {
      expect(call.mock.calls[0]).toContain("proxima-centauri");
    }
  });
});
