/** @vitest-environment jsdom */
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, renderHook, waitFor } from "@testing-library/react";
import {
  QueryClient,
  QueryClientProvider,
  useMutation,
} from "@tanstack/react-query";
import type { ReactNode } from "react";
import { createIssueViewStore } from "@patchbay/core/issues/stores/view-store";
import { useIssueSurfaceActions } from "./use-issue-surface-actions";

const request = vi.hoisted(() => vi.fn());
const rollback = vi.hoisted(() => vi.fn());
vi.mock("@patchbay/core/issues/mutations", () => ({
  useUpdateIssue: () => useMutation({ mutationFn: request, onError: rollback }),
  useBatchUpdateIssues: () => ({ mutateAsync: vi.fn() }),
  useBatchDeleteIssues: () => ({ mutateAsync: vi.fn() }),
}));
vi.mock("../../i18n", () => ({ useT: () => ({ t: () => "move failed" }) }));
vi.mock("sonner", () => ({ toast: { error: vi.fn() } }));

function deferred() {
  let resolve!: () => void;
  let reject!: (error: Error) => void;
  const promise = new Promise<void>((yes, no) => {
    resolve = yes;
    reject = no;
  });
  return { promise, resolve, reject };
}

function setup() {
  const client = new QueryClient({
    defaultOptions: { mutations: { retry: false } },
  });
  const wrapper = ({ children }: { children: ReactNode }) => (
    <QueryClientProvider client={client}>{children}</QueryClientProvider>
  );
  return renderHook(() => useIssueSurfaceActions({ createDefaults: {} }), {
    wrapper,
  });
}

describe("board move callbacks", () => {
  beforeEach(() => {
    request.mockReset();
    rollback.mockReset();
  });
  afterEach(cleanup);

  it("restores the original persistent view after a failed move finishes following unmount", async () => {
    const pending = deferred();
    request.mockReturnValue(pending.promise);
    const store = createIssueViewStore("review-hidden-column-unmount");
    store.getState().hideStatus("done");
    const settled = vi.fn();
    const restored = vi.fn(() => {
      expect(rollback).toHaveBeenCalledOnce();
      store.getState().hideStatus("done");
    });
    const hook = setup();
    act(() => {
      hook.result.current.moveIssue(
        "issue-1",
        { status: "done", before_id: null, after_id: null },
        { onError: restored, onSettled: settled },
      );
      store.getState().showStatus("done");
    });
    await waitFor(() => expect(request).toHaveBeenCalledOnce());
    hook.unmount();
    await act(async () => {
      pending.reject(new Error("move rejected"));
    });
    await waitFor(() => expect(restored).toHaveBeenCalledOnce());
    expect(settled).toHaveBeenCalledOnce();
    expect(store.getState().hiddenStatusCategories).toContain("done");
  });

  it("settles both overlapping moves even when the observer tracks a newer request", async () => {
    const first = deferred(),
      second = deferred();
    request
      .mockReturnValueOnce(first.promise)
      .mockReturnValueOnce(second.promise);
    const firstSuccess = vi.fn(),
      secondSuccess = vi.fn();
    const firstSettled = vi.fn(),
      secondSettled = vi.fn();
    const hook = setup();
    act(() =>
      hook.result.current.moveIssue(
        "first",
        { status: "done", before_id: null, after_id: null },
        { onSuccess: firstSuccess, onSettled: firstSettled },
      ),
    );
    await waitFor(() => expect(request).toHaveBeenCalledTimes(1));
    act(() =>
      hook.result.current.moveIssue(
        "second",
        { status: "done", before_id: null, after_id: null },
        { onSuccess: secondSuccess, onSettled: secondSettled },
      ),
    );
    await waitFor(() => expect(request).toHaveBeenCalledTimes(2));
    hook.unmount();
    await act(async () => {
      second.resolve();
      first.resolve();
    });
    await waitFor(() => expect(firstSuccess).toHaveBeenCalledOnce());
    expect(secondSuccess).toHaveBeenCalledOnce();
    expect(firstSettled).toHaveBeenCalledOnce();
    expect(secondSettled).toHaveBeenCalledOnce();
  });
});
