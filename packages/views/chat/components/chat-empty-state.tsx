"use client";

import { Flexbox } from "@lobehub/ui/es/Flex/index";
import type { Agent } from "@patchbay/core/types";
import { cn } from "@patchbay/ui/lib/utils";
import { useT } from "../../i18n";
import { AgentAcpAvatar } from "../../runtimes/components/acp-avatar";
import { CHAT_COLUMN, CHAT_GUTTER } from "./chat-column";

/**
 * Agent-page welcome, mapped from official https://github.com/lobehub/lobehub
 * `src/features/AgentHome/AgentInfo.tsx`: 64px square runtime logo, 24px bold
 * name, then a greeting. Opening-question chips are intentionally omitted.
 */
export function EmptyState({ agent }: { agent: Agent | null }) {
  const { t } = useT("chat");
  const description = agent?.description?.trim();
  const greeting = description
    ? description
    : agent
      ? t(($) => $.empty_state.greeting, { name: agent.name })
      : null;

  return (
    <div className={cn("flex min-h-0 flex-1 flex-col overflow-y-auto", CHAT_GUTTER)}>
      <Flexbox className={cn(CHAT_COLUMN, "min-h-full")} flex={1} width="100%">
        <Flexbox flex={1} />
        <Flexbox gap={32} style={{ paddingBottom: "max(4vh, 16px)" }} width="100%">
          {agent ? (
            <Flexbox gap={12}>
              <AgentAcpAvatar
                agentId={agent.id}
                size={64}
                shape="square"
                name={agent.name}
              />
              <h3 className="text-display-sm font-bold leading-tight">{agent.name}</h3>
              {greeting ? (
                <p className="max-w-[640px] text-body text-muted-foreground">{greeting}</p>
              ) : null}
            </Flexbox>
          ) : (
            <Flexbox gap={12}>
              <h3 className="text-display-sm font-bold leading-tight">
                {t(($) => $.empty_state.first_time_title)}
              </h3>
              <p className="max-w-[640px] text-body text-muted-foreground">
                {t(($) => $.empty_state.first_time_intro)}{" "}
                <span className="font-medium text-foreground">
                  {t(($) => $.empty_state.first_time_pillars)}
                </span>
                {t(($) => $.empty_state.first_time_pillars_suffix)}
              </p>
              <p className="max-w-[640px] text-body text-muted-foreground">
                {t(($) => $.empty_state.first_time_actions)}
              </p>
            </Flexbox>
          )}
        </Flexbox>
      </Flexbox>
    </div>
  );
}
