import { describe, expect, it } from "vitest";
import { isPatrickAgent, memberNeedsPatrickSetup, workspaceNeedsPatrick } from "./patrick";

const patrick = { id: "agent-patrick", system_key: "patrick" };
const other = { id: "agent-other", system_key: undefined };
const kicked = {
  agent_id: "agent-patrick",
  last_message: { content: "…", role: "user" as const, created_at: "2026-01-01" },
};

describe("workspaceNeedsPatrick", () => {
  it("is true for an empty workspace", () => {
    expect(workspaceNeedsPatrick([])).toBe(true);
  });

  // The regression this guards: the Runtimes recovery card gated on
  // `agents.length === 0`, so creating any ordinary agent first hid the only
  // surface that can mint a Patrick. The generic agent endpoint accepts neither
  // `kind` nor `system_key`, so there was no other way back.
  it("stays true when the workspace has ordinary agents but no Patrick", () => {
    expect(
      workspaceNeedsPatrick([{ system_key: undefined }, { system_key: "" }]),
    ).toBe(true);
  });

  it("is false once a Patrick exists", () => {
    expect(workspaceNeedsPatrick([{ system_key: "patrick" }])).toBe(false);
    expect(
      workspaceNeedsPatrick([{ system_key: undefined }, { system_key: "patrick" }]),
    ).toBe(false);
  });

  // Identity is the system key, never the display name — Patrick is renameable.
  it("does not treat a renamed-to-Patrick ordinary agent as Patrick", () => {
    expect(isPatrickAgent({ system_key: undefined })).toBe(false);
    expect(isPatrickAgent({ system_key: "agent_builder" })).toBe(false);
  });
});

// Bootstrapping is three server steps and the last two can fail after the
// agent commits. Gating the entrypoint on the agent alone meant the agent's
// own `agent:created` broadcast tore the card down the moment step one
// succeeded — and it never returned, because the agent is durable and the rest
// was not. These cases are exactly the partial states that produced.
describe("memberNeedsPatrickSetup", () => {
  it("is true when the workspace has no Patrick at all", () => {
    expect(memberNeedsPatrickSetup([other], [])).toBe(true);
  });

  it("stays true when the agent exists but the member has no session", () => {
    expect(memberNeedsPatrickSetup([patrick], [])).toBe(true);
  });

  it("stays true when the session exists but the kickoff never landed", () => {
    expect(
      memberNeedsPatrickSetup([patrick], [{ agent_id: "agent-patrick", last_message: null }]),
    ).toBe(true);
  });

  // Sessions are per member, so another agent's conversation is not this one.
  it("ignores sessions belonging to other agents", () => {
    expect(
      memberNeedsPatrickSetup([patrick], [{ ...kicked, agent_id: "agent-other" }]),
    ).toBe(true);
  });

  it("is false once the member has a kicked-off Patrick conversation", () => {
    expect(memberNeedsPatrickSetup([other, patrick], [kicked])).toBe(false);
  });
});
