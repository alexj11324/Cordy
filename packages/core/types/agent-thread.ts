import type { AgentTask } from "./agent";
import type { TaskMessagePayload } from "./events";

/** Provider-neutral availability of a persisted Agent thread. */
export type AgentThreadAvailabilityState = "available" | "unavailable";

export interface AgentThreadAvailability {
  state: AgentThreadAvailabilityState;
  reason_code?: string;
  reason?: string;
}

export interface AgentThreadAgent {
  id: string;
  name: string;
  avatar_url: string | null;
}

export interface AgentThreadResponse {
  /** The newest task in the provider-session chain; sends target this task. */
  task: AgentTask;
  /** All turns in creation order, including the task used to open the thread. */
  thread_tasks: AgentTask[];
  current_task_id: string;
  agent: AgentThreadAgent;
  /** Structured, public task events; internal reasoning is never included. */
  events: TaskMessagePayload[];
  availability: AgentThreadAvailability;
  can_continue: boolean;
}

export interface ContinueAgentThreadRequest {
  content: string;
  idempotency_key: string;
}

export interface ContinueAgentThreadResponse {
  continuation_task_id: string;
  status: "queued" | "coalesced";
}
