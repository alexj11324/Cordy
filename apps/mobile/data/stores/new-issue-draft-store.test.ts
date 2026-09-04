import { afterEach, describe, expect, it } from "vitest";
import { useNewIssueDraftStore } from "./new-issue-draft-store";

afterEach(() => useNewIssueDraftStore.getState().reset());

describe("new issue review handoff draft", () => {
  it("sets status and reviewer atomically without changing owner or executor", () => {
    const store = useNewIssueDraftStore.getState();
    store.setOwner({ type: "member", id: "owner-1" });
    store.setExecutor({ type: "agent", id: "agent-1" });

    store.setReviewHandoff("quality-review", {
      type: "member",
      id: "reviewer-1",
    });

    expect(useNewIssueDraftStore.getState()).toMatchObject({
      status: "quality-review",
      owner: { type: "member", id: "owner-1" },
      executor: { type: "agent", id: "agent-1" },
      reviewer: { type: "member", id: "reviewer-1" },
    });
  });
});
