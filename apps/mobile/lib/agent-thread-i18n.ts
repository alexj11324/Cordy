import { useMemo } from "react";
import { useAuthStore } from "@/data/auth-store";
import en from "@/locales/en/agent-thread";
import zhHans from "@/locales/zh-Hans/agent-thread";
import ja from "@/locales/ja/agent-thread";
import ko from "@/locales/ko/agent-thread";

export type MobileLocale = "en" | "zh-Hans" | "ja" | "ko";
export type AgentThreadCopy = typeof en;

const RESOURCES = {
  en,
  "zh-Hans": zhHans,
  ja,
  ko,
} satisfies Record<MobileLocale, AgentThreadCopy>;

export function normalizeMobileLocale(
  language: string | null | undefined,
): MobileLocale {
  const normalized = language?.trim().toLowerCase() ?? "";
  if (normalized === "zh-hans" || normalized.startsWith("zh")) return "zh-Hans";
  if (normalized.startsWith("ja")) return "ja";
  if (normalized.startsWith("ko")) return "ko";
  return "en";
}

export function formatAgentThreadCopy(
  template: string,
  values: Record<string, string>,
): string {
  return template.replace(
    /\{\{(\w+)\}\}/g,
    (_, key: string) => values[key] ?? "",
  );
}

/** Prefer a localized stable reason code over backend English copy. */
export function agentThreadAvailabilityMessage(
  copy: AgentThreadCopy,
  reasonCode: string | undefined,
  serverReason: string | undefined,
  fallback = copy.unavailable_fallback,
): string {
  const localized: Record<string, string> = {
    provider_session_retired: copy.reason_provider_session_retired,
    provider_session_missing: copy.reason_provider_session_missing,
    fresh_session_required: copy.reason_fresh_session_required,
    provider_session_not_established:
      copy.reason_provider_session_not_established,
    agent_archived: copy.reason_agent_archived,
    agent_runtime_unbound: copy.reason_agent_runtime_unbound,
    agent_runtime_rebound: copy.reason_agent_runtime_rebound,
    agent_runtime_missing: copy.reason_agent_runtime_missing,
    agent_thread_invoke_forbidden: copy.reason_agent_thread_invoke_forbidden,
  };
  return (reasonCode && localized[reasonCode]) || serverReason || fallback;
}

export function useAgentThreadCopy(): AgentThreadCopy {
  const language = useAuthStore((state) => state.user?.language);
  return useMemo(() => RESOURCES[normalizeMobileLocale(language)], [language]);
}
