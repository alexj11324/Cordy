// @vitest-environment node
import { describe, expect, it } from "vitest";
import {
  handoffHopsForDisplay,
  handoffStackActors,
  issueActor,
  reviewHandoffHops,
} from "./handoff-chain";

describe("reviewHandoffHops", () => {
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
        {
          action: "review_handoff",
          details: { from_type: "agent", from_id: "research" },
        },
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
      {
        from: { type: "agent", id: "content" },
        to: { type: "agent", id: "research" },
      },
      {
        from: { type: "agent", id: "research" },
        to: { type: "agent", id: "coding" },
      },
    ]);
  });
});

describe("handoffStackActors", () => {
  it("dedupes in chain order and appends current assignee and reviewer", () => {
    const hops = reviewHandoffHops([
      {
        action: "review_handoff",
        details: {
          from_type: "agent",
          from_id: "content",
          to_type: "agent",
          to_id: "research",
        },
      },
      {
        action: "review_handoff",
        details: {
          from_type: "agent",
          from_id: "research",
          to_type: "agent",
          to_id: "coding",
        },
      },
    ]);
    expect(
      handoffStackActors(
        hops,
        issueActor("agent", "coding"),
        issueActor("agent", "research"),
      ),
    ).toEqual([
      { type: "agent", id: "content" },
      { type: "agent", id: "research" },
      { type: "agent", id: "coding" },
    ]);
  });
});

describe("handoffHopsForDisplay", () => {
  it("synthesizes a hop when history is empty but both roles are set", () => {
    expect(
      handoffHopsForDisplay(
        [],
        issueActor("agent", "coding"),
        issueActor("agent", "research"),
      ),
    ).toEqual([
      {
        from: { type: "agent", id: "coding" },
        to: { type: "agent", id: "research" },
      },
    ]);
  });

  it("prefers recorded hops over a synthetic current pair", () => {
    const hops = [
      {
        from: { type: "agent" as const, id: "content" },
        to: { type: "agent" as const, id: "research" },
      },
    ];
    expect(
      handoffHopsForDisplay(
        hops,
        issueActor("agent", "coding"),
        issueActor("agent", "research"),
      ),
    ).toEqual(hops);
  });
});
