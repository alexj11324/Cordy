export const channelKeys = {
  all: (wsId: string) => ["workspaces", wsId, "channels"] as const,
  list: (wsId: string) => [...channelKeys.all(wsId), "list"] as const,
  detail: (wsId: string, channelId: string) =>
    [...channelKeys.all(wsId), "detail", channelId] as const,
  messages: (wsId: string, channelId: string) =>
    [...channelKeys.all(wsId), "messages", channelId] as const,
};
