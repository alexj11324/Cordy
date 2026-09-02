import type { AgentTask } from "./agent";
import type { TaskMessagePayload } from "./events";

export type AgentThreadAvailability = {
  state: "available" | "unavailable";
  reason_code?: string;
  reason?: string;
};

export type AgentThreadResponse = {
  task: AgentTask;
  thread_tasks: AgentTask[];
  current_task_id: string;
  agent: {
    id: string;
    name: string;
    avatar_url: string | null;
  };
  events: TaskMessagePayload[];
  availability: AgentThreadAvailability;
  can_continue: boolean;
};

export type ContinueAgentThreadRequest = {
  content: string;
  idempotency_key: string;
};

export type ContinueAgentThreadResponse = {
  continuation_task_id: string;
  status: "queued" | "coalesced";
};
