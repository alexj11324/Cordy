"use client";

import type { Agent } from "@patchbay/core/types";
import { cn } from "@patchbay/ui/lib/utils";
import { ActorAvatar } from "../../common/actor-avatar";
import { useT } from "../../i18n";

/** Empty compose placeholder shown before the first user message. */
export function EmptyState({ agent }: { agent: Agent | null }) {
  const { t } = useT("chat");
  const description = agent?.description?.trim();

  return (
    <div
      className={cn(
        "flex min-h-0 flex-1 px-8 py-10",
        agent ? "items-end" : "items-center justify-center",
      )}
    >
      {agent && (
        <div className="w-full max-w-5xl pb-2">
          <ActorAvatar
            actorType="agent"
            actorId={agent.id}
            size="2xl"
            className="mb-5 ring-1 ring-inset ring-border"
          />
          <div className="max-w-2xl space-y-1 text-left">
            <h3 className="text-title-sm font-semibold">
              {t(($) => $.empty_state.chat_with_named, { name: agent.name })}
            </h3>
            {description && (
              <p className="text-body text-muted-foreground">{description}</p>
            )}
          </div>
        </div>
      )}
      {!agent && (
        <div className="max-w-sm space-y-1 text-center">
          <h3 className="text-title-sm font-semibold">
            {t(($) => $.empty_state.first_time_title)}
          </h3>
        </div>
      )}
    </div>
  );
}
