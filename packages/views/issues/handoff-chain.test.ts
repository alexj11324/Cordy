// @vitest-environment node
import { describe, expect, it } from "vitest";
import {
  handoffHopsForDisplay,
  handoffStackActors,
  issueActor,
  reviewHandoffHops,
} from "./handoff-chain";

describe("review handoff chain", () => {
  it("keeps chronological hops and skips malformed rows", () => {
    expect(
      reviewHandoffHops([
        { action: "created" },
        {
          action: "review_handoff",
          details: {
            from_type: "agent",
            from_id: "content",
            to_type: "agent",
            to_id: "research",
          },
        },
        { action: "review_handoff", details: { from_type: "agent", from_id: "research" } },
        {
          action: "review_handoff",
          details: {
            from_type: "agent",
            from_id: "research",
            to_type: "agent",
            to_id: "coding",
          },
        },
      ]),
    ).toEqual([
      { from: { type: "agent", id: "content" }, to: { type: "agent", id: "research" } },
      { from: { type: "agent", id: "research" }, to: { type: "agent", id: "coding" } },
    ]);
  });

  it("dedupes actors in chain order and appends current roles", () => {
    const hops = reviewHandoffHops([
      {
        action: "review_handoff",
        details: { from_type: "agent", from_id: "content", to_type: "agent", to_id: "research" },
      },
      {
        action: "review_handoff",
        details: { from_type: "agent", from_id: "research", to_type: "agent", to_id: "coding" },
      },
    ]);
    expect(
      handoffStackActors(hops, issueActor("agent", "coding"), issueActor("agent", "research")),
    ).toEqual([
      { type: "agent", id: "content" },
      { type: "agent", id: "research" },
      { type: "agent", id: "coding" },
    ]);
  });

  it("synthesizes a hop when current roles exist without history", () => {
    expect(
      handoffHopsForDisplay([], issueActor("agent", "coding"), issueActor("agent", "research")),
    ).toEqual([
      { from: { type: "agent", id: "coding" }, to: { type: "agent", id: "research" } },
    ]);
  });
});
