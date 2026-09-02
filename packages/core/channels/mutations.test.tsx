/**
 * @vitest-environment jsdom
 */
import { act, renderHook, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import type { ReactNode } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { setApiInstance } from "../api";
import type { ApiClient } from "../api/client";
import type { WorkspaceChannelMessage } from "../types/channel";
import { useCreateWorkspaceChannelMessage } from "./mutations";
import { channelKeys } from "./keys";

vi.mock("../hooks", () => ({
  useWorkspaceId: () => "ws-1",
}));

const message: WorkspaceChannelMessage = {
  id: "message-1",
  workspace_id: "ws-1",
  channel_id: "channel-1",
  author_type: "member",
  author_id: "member-1",
  content: "Ship the first pass.",
  parent_id: null,
  quoted_message_id: null,
  created_at: "2026-09-02T00:01:00Z",
  updated_at: "2026-09-02T00:01:00Z",
};

function wrapper(queryClient: QueryClient) {
  return function Wrapper({ children }: { children: ReactNode }) {
    return <QueryClientProvider client={queryClient}>{children}</QueryClientProvider>;
  };
}

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason?: unknown) => void;
  const promise = new Promise<T>((resolvePromise, rejectPromise) => {
    resolve = resolvePromise;
    reject = rejectPromise;
  });
  return { promise, resolve, reject };
}

describe("channel message mutation cache", () => {
  let queryClient: QueryClient;

  beforeEach(() => {
    queryClient = new QueryClient({
      defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
    });
  });

  afterEach(() => {
    queryClient.clear();
    vi.restoreAllMocks();
  });

  it("shows an optimistic message and reconciles the server response", async () => {
    const response = deferred<WorkspaceChannelMessage>();
    const createMessage = vi.fn(() => response.promise);
    setApiInstance({ createWorkspaceChannelMessage: createMessage } as unknown as ApiClient);
    const key = channelKeys.messages("ws-1", "channel-1");
    queryClient.setQueryData(key, { messages: [] });

    const { result } = renderHook(() => useCreateWorkspaceChannelMessage(), {
      wrapper: wrapper(queryClient),
    });
    let request!: Promise<WorkspaceChannelMessage>;
    act(() => {
      request = result.current.mutateAsync({
        channelId: "channel-1",
        author_type: "member",
        author_id: "member-1",
        content: message.content,
      });
    });

    await waitFor(() => {
      const cached = queryClient.getQueryData<{
        messages: Array<WorkspaceChannelMessage & { optimistic?: boolean }>;
      }>(key);
      expect(cached?.messages[0]?.optimistic).toBe(true);
    });

    await act(async () => {
      response.resolve(message);
      await request;
    });

    const cached = queryClient.getQueryData<{
      messages: Array<WorkspaceChannelMessage & { optimistic?: boolean }>;
    }>(key);
    expect(cached?.messages).toEqual([message]);
    expect(cached?.messages.some((item) => item.optimistic)).toBe(false);
  });

  it("removes only the failed optimistic row and keeps existing messages", async () => {
    const existing: WorkspaceChannelMessage = { ...message, id: "existing", content: "Earlier" };
    const createMessage = vi.fn(() => Promise.reject(new Error("offline")));
    setApiInstance({ createWorkspaceChannelMessage: createMessage } as unknown as ApiClient);
    const key = channelKeys.messages("ws-1", "channel-1");
    queryClient.setQueryData(key, { messages: [existing] });

    const { result } = renderHook(() => useCreateWorkspaceChannelMessage(), {
      wrapper: wrapper(queryClient),
    });
    await act(async () => {
      await expect(
        result.current.mutateAsync({
          channelId: "channel-1",
          author_type: "member",
          author_id: "member-1",
          content: message.content,
        }),
      ).rejects.toThrow("offline");
    });

    expect(queryClient.getQueryData(key)).toEqual({ messages: [existing] });
  });

  it("rolls back only the optimistic row in an infinite transcript", async () => {
    const existing: WorkspaceChannelMessage = { ...message, id: "existing", content: "Earlier" };
    const createMessage = vi.fn(() => Promise.reject(new Error("offline")));
    setApiInstance({ createWorkspaceChannelMessage: createMessage } as unknown as ApiClient);
    const key = channelKeys.messages("ws-1", "channel-1");
    queryClient.setQueryData(key, {
      pages: [
        { messages: [existing], limit: 50, has_more: true, next_cursor: null },
      ],
      pageParams: [null],
    });

    const { result } = renderHook(() => useCreateWorkspaceChannelMessage(), {
      wrapper: wrapper(queryClient),
    });
    await act(async () => {
      await expect(
        result.current.mutateAsync({
          channelId: "channel-1",
          author_type: "member",
          author_id: "member-1",
          content: message.content,
        }),
      ).rejects.toThrow("offline");
    });

    expect(queryClient.getQueryData(key)).toEqual({
      pages: [
        { messages: [existing], limit: 50, has_more: true, next_cursor: null },
      ],
      pageParams: [null],
    });
  });

  it("reconciles a successful response when the server derives a different actor", async () => {
    const response = deferred<WorkspaceChannelMessage>();
    const createMessage = vi.fn(() => response.promise);
    setApiInstance({ createWorkspaceChannelMessage: createMessage } as unknown as ApiClient);
    const key = channelKeys.messages("ws-1", "channel-1");
    queryClient.setQueryData(key, { messages: [] });

    const { result } = renderHook(() => useCreateWorkspaceChannelMessage(), {
      wrapper: wrapper(queryClient),
    });
    let request!: Promise<WorkspaceChannelMessage>;
    act(() => {
      request = result.current.mutateAsync({
        channelId: "channel-1",
        author_type: "member",
        author_id: "member-1",
        content: message.content,
      });
    });

    await waitFor(() => {
      expect(
        queryClient.getQueryData<{
          messages: Array<WorkspaceChannelMessage & { optimistic?: boolean }>;
        }>(key)?.messages[0]?.optimistic,
      ).toBe(true);
    });

    const serverMessage: WorkspaceChannelMessage = {
      ...message,
      id: "agent-message-1",
      author_type: "agent",
      author_id: "agent-1",
    };
    await act(async () => {
      response.resolve(serverMessage);
      await request;
    });

    expect(queryClient.getQueryData(key)).toEqual({ messages: [serverMessage] });
  });

  it("removes a seeded optimistic cache when the transcript was not loaded", async () => {
    const createMessage = vi.fn(() => Promise.reject(new Error("offline")));
    setApiInstance({ createWorkspaceChannelMessage: createMessage } as unknown as ApiClient);

    const { result } = renderHook(() => useCreateWorkspaceChannelMessage(), {
      wrapper: wrapper(queryClient),
    });

    await act(async () => {
      await expect(
        result.current.mutateAsync({
          channelId: "channel-1",
          author_type: "member",
          author_id: "member-1",
          content: message.content,
        }),
      ).rejects.toThrow("offline");
    });

    expect(
      queryClient.getQueryData(channelKeys.messages("ws-1", "channel-1")),
    ).toBeUndefined();
  });

  it("rolls back when a successful HTTP response is malformed", async () => {
    const createMessage = vi.fn(() =>
      Promise.resolve({ ...message, id: "" }),
    );
    setApiInstance({ createWorkspaceChannelMessage: createMessage } as unknown as ApiClient);

    const { result } = renderHook(() => useCreateWorkspaceChannelMessage(), {
      wrapper: wrapper(queryClient),
    });

    await act(async () => {
      await expect(
        result.current.mutateAsync({
          channelId: "channel-1",
          author_type: "member",
          author_id: "member-1",
          content: message.content,
        }),
      ).rejects.toThrow("Invalid workspace channel message response");
    });

    expect(
      queryClient.getQueryData(channelKeys.messages("ws-1", "channel-1")),
    ).toBeUndefined();
  });
});
