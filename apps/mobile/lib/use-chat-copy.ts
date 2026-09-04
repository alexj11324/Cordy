import { useMemo } from "react";
import { useAuthStore } from "@/data/auth-store";
import { createChatCopy } from "@/lib/chat-copy";

/** Mobile chat's narrow i18n adapter. The account language is already shared
 * by the Go API and the web/desktop locale settings; chat is the only mobile
 * surface currently opting into it. */
export function useChatCopy() {
  const language = useAuthStore((state) => state.user?.language);
  return useMemo(() => createChatCopy(language), [language]);
}
