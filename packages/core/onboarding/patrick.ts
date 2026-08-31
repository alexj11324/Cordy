import type { Agent, ChatSession } from "../types";

/**
 * Mirrors `service.PatrickSystemKey` on the server. Patrick is identified by this
 * key and never by display name — the name is owner-editable, so a rename
 * would otherwise make the workspace look like it has no Patrick.
 */
export const PATRICK_SYSTEM_KEY = "patrick";

export function isPatrickAgent(agent: Pick<Agent, "system_key">): boolean {
  return agent.system_key === PATRICK_SYSTEM_KEY;
}

/**
 * Whether the workspace still needs a Patrick provisioned.
 *
 * Deliberately "no Patrick" rather than "no agents at all": the recovery
 * entrypoint on the Runtimes page used the latter, so creating any ordinary
 * agent first hid the only surface that can mint a Patrick — and the generic
 * agent endpoint cannot, since it accepts neither `kind` nor `system_key`.
 */
export function workspaceNeedsPatrick(agents: Pick<Agent, "system_key">[]): boolean {
  return !agents.some(isPatrickAgent);
}

/**
 * Whether *this member* still needs the "Start with Patrick" entrypoint.
 *
 * Not the same question as `workspaceNeedsPatrick`, and gating the entrypoint on
 * that one was a trap: bootstrapping is three server steps — provision the
 * agent, open the member's session, enqueue the opening turn — and the last
 * two can fail after the agent has committed. The agent's own `agent:created`
 * broadcast then invalidates the agent list, so the card unmounted (taking its
 * open dialog with it) the instant the *first* step succeeded, and never came
 * back on reload, because the agent is durable and the rest was not. The
 * member was left with a Patrick they could not start.
 *
 * Every step is idempotent, so the honest condition is "has this member
 * actually ended up with a Patrick conversation that has been kicked off" — and
 * re-running the flow from here is always safe.
 */
export function memberNeedsPatrickSetup(
  agents: Pick<Agent, "id" | "system_key">[],
  sessions: Pick<ChatSession, "agent_id" | "last_message">[],
): boolean {
  const patrick = agents.find(isPatrickAgent);
  if (!patrick) return true;

  // Sessions are per member, so this is the caller's own conversation.
  const session = sessions.find((s) => s.agent_id === patrick.id);
  if (!session) return true;

  // The opening turn is stored as a real (hidden) message, so an empty
  // conversation means the kickoff never landed.
  return !session.last_message;
}
