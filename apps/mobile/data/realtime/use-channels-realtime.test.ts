import { beforeEach, describe, expect, it, vi } from "vitest";

const { invalidateQueries, getQueryData, setQueryData, setups } = vi.hoisted(() => ({
  invalidateQueries: vi.fn(),
  getQueryData: vi.fn(),
  setQueryData: vi.fn(),
  setups: [] as Array<(ws: MockWS, wsId: string) => Array<() => void>>,
}));

type Handler = (payload: unknown) => void;

interface MockWS {
  on: ReturnType<typeof vi.fn>;
  onReconnect: ReturnType<typeof vi.fn>;
}

vi.mock("@tanstack/react-query", () => ({
  useQueryClient: () => ({ invalidateQueries, getQueryData, setQueryData }),
}));
vi.mock("@/lib/use-ws-subscriptions", () => ({
  useWSSubscriptions: (
    setup: (ws: MockWS, wsId: string) => Array<() => void>,
  ) => setups.push(setup),
}));
vi.mock("@/data/api", () => ({ api: {} }));

import { channelKeys } from "@/data/queries/channels";
import { useChannelsRealtime } from "./use-channels-realtime";

const MESSAGE = {
  id: "message-1",
  workspace_id: "workspace-1",
  channel_id: "channel-1",
  author_type: "member",
  author_id: "user-1",
  content: "Hello",
  parent_id: null,
  quoted_message_id: null,
  created_at: "2026-09-02T00:00:00Z",
  updated_at: "2026-09-02T00:00:00Z",
};

describe("useChannelsRealtime", () => {
  beforeEach(() => {
    invalidateQueries.mockReset();
    getQueryData.mockReset();
    setQueryData.mockReset();
    setups.length = 0;
  });

  it("filters foreign events and invalidates the unseen list on a valid create", () => {
    useChannelsRealtime();
    expect(setups).toHaveLength(1);

    const handlers = new Map<string, Handler>();
    const ws: MockWS = {
      on: vi.fn((event: string, handler: Handler) => {
        handlers.set(event, handler);
        return () => {};
      }),
      onReconnect: vi.fn((handler: () => void) => {
        handlers.set("reconnect", handler);
        return () => {};
      }),
    };
    setups[0](ws, "workspace-1");
    getQueryData.mockReturnValue(undefined);

    handlers.get("channel:created")?.({
      id: "foreign-channel",
      workspace_id: "workspace-2",
      slug: "foreign",
      name: "Foreign",
    });
    expect(invalidateQueries).not.toHaveBeenCalled();

    handlers.get("channel:created")?.({
      id: "channel-1",
      workspace_id: "workspace-1",
      slug: "updates",
      name: "Updates",
    });
    expect(invalidateQueries).toHaveBeenCalledWith({
      queryKey: channelKeys.list("workspace-1"),
    });
  });

  it("patches a valid message event and refetches all channel caches after reconnect", () => {
    useChannelsRealtime();
    const handlers = new Map<string, Handler>();
    const ws: MockWS = {
      on: vi.fn((event: string, handler: Handler) => {
        handlers.set(event, handler);
        return () => {};
      }),
      onReconnect: vi.fn((handler: () => void) => {
        handlers.set("reconnect", handler);
        return () => {};
      }),
    };
    setups[0](ws, "workspace-1");

    handlers.get("channel:message")?.({ message: MESSAGE });
    expect(setQueryData).toHaveBeenCalled();
    expect(invalidateQueries).not.toHaveBeenCalled();

    handlers.get("reconnect")?.(undefined);
    expect(invalidateQueries).toHaveBeenCalledWith({
      queryKey: channelKeys.all("workspace-1"),
    });
  });
});
