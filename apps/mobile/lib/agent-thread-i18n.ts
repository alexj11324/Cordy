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

export function normalizeMobileLocale(language: string | null | undefined): MobileLocale {
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
  return template.replace(/\{\{(\w+)\}\}/g, (_, key: string) => values[key] ?? "");
}

export function useAgentThreadCopy(): AgentThreadCopy {
  const language = useAuthStore((state) => state.user?.language);
  return useMemo(
    () => RESOURCES[normalizeMobileLocale(language)],
    [language],
  );
}
