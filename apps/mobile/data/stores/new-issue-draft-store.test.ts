import { afterEach, describe, expect, it } from "vitest";
import { useNewIssueDraftStore } from "./new-issue-draft-store";

afterEach(() => useNewIssueDraftStore.getState().reset());

describe("new issue review handoff draft", () => {
  it("sets status and reviewer atomically without changing owner or executor", () => {
    const store = useNewIssueDraftStore.getState();
    store.setOwner({ type: "member", id: "owner-1" });
    store.setExecutor({ type: "agent", id: "agent-1" });

    store.setReviewHandoff(
      "quality-review",
      {
        type: "member",
        id: "reviewer-1",
      },
      {
        worktree: "/worktrees/issue-42",
        branch: "codex/issue-42",
        commit: "a".repeat(40),
        pull_requests: ["https://github.com/example/repo/pull/42"],
      },
    );

    expect(useNewIssueDraftStore.getState()).toMatchObject({
      status: "quality-review",
      owner: { type: "member", id: "owner-1" },
      executor: { type: "agent", id: "agent-1" },
      reviewer: { type: "member", id: "reviewer-1" },
      reviewSubmission: {
        worktree: "/worktrees/issue-42",
        branch: "codex/issue-42",
        commit: "a".repeat(40),
        pull_requests: ["https://github.com/example/repo/pull/42"],
      },
    });
  });

  it("clears review evidence when the draft leaves review", () => {
    const store = useNewIssueDraftStore.getState();
    store.setReviewHandoff(
      "in_review",
      { type: "member", id: "reviewer-1" },
      {
        worktree: "/worktrees/issue-42",
        branch: "codex/issue-42",
        commit: "a".repeat(40),
        pull_requests: ["https://github.com/example/repo/pull/42"],
      },
    );

    store.setStatus("todo");

    expect(useNewIssueDraftStore.getState().reviewSubmission).toBeNull();
  });

  it("clears review evidence when either review participant changes", () => {
    const store = useNewIssueDraftStore.getState();
    const submission = {
      worktree: "/worktrees/issue-42",
      branch: "codex/issue-42",
      commit: "a".repeat(40),
      pull_requests: ["https://github.com/example/repo/pull/42"],
    };
    store.setReviewHandoff("in_review", { type: "member", id: "reviewer-1" }, submission);
    store.setExecutor({ type: "agent", id: "agent-2" });
    expect(useNewIssueDraftStore.getState().reviewSubmission).toBeNull();

    store.setReviewHandoff("in_review", { type: "member", id: "reviewer-1" }, submission);
    store.setReviewer({ type: "member", id: "reviewer-2" });
    expect(useNewIssueDraftStore.getState().reviewSubmission).toBeNull();
  });
});
