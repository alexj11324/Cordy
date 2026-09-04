export const NEW_CHAT_TITLE = "New chat";

export function chatSessionDisplayTitle(
  title: string | null | undefined,
  fallback = NEW_CHAT_TITLE,
): string {
  return title || fallback;
}
