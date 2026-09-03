import { useMemo } from "react";
import { useAuthStore } from "@/data/auth-store";
import {
  getAgentThreadCopy,
  type AgentThreadCopy,
} from "@/lib/agent-thread-i18n";

export function useAgentThreadCopy(): AgentThreadCopy {
  const language = useAuthStore((state) => state.user?.language);
  return useMemo(() => getAgentThreadCopy(language), [language]);
}
