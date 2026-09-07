import type { Agent, AgentConversationStarter } from "../types";

/**
 * Resolve what a new chat with this agent actually shows.
 *
 * A configured suggestion only counts once BOTH halves carry text: the editor
 * lets an author add a row and save is blocked while it is half-filled, but a
 * malformed payload from an older client must not render a blank button.
 *
 * When nothing complete survives, the caller's localized fallbacks are used
 * and `isFallback` says so — the settings preview needs to tell an author
 * "these are the defaults" rather than implying they configured them.
 */
export function selectConversationStarters(
  configured: AgentConversationStarter[] | undefined | null,
  fallback: AgentConversationStarter[],
): { starters: AgentConversationStarter[]; isFallback: boolean } {
  const complete = (configured ?? []).filter(
    (item) => item.label.trim() !== "" && item.prompt.trim() !== "",
  );
  return complete.length > 0
    ? { starters: complete, isFallback: false }
    : { starters: fallback, isFallback: true };
}

/**
 * Whether this viewer should be offered a way to edit the agent's conversation
 * starters from the chat empty state they render in.
 *
 * The Instructions tab that hosted that editor is gone: starters can still be
 * set at create time, but a "customize" link would land on a missing view.
 */
export function canCustomizeConversationStarters(
  _agent: Pick<Agent, "archived_at"> | null,
  _opts: { conversationStartersSupported: boolean; canEditAgent: boolean },
): boolean {
  return false;
}
