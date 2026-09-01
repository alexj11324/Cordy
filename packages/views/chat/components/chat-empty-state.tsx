"use client";

import { useCallback } from "react";
import { Flexbox } from "@lobehub/ui/es/Flex/index";
import type { Agent } from "@patchbay/core/types";
import { DRAFT_NEW_SESSION, useChatStore } from "@patchbay/core/chat";
import { cn } from "@patchbay/ui/lib/utils";
import { ActorAvatar } from "../../common/actor-avatar";
import { useT } from "../../i18n";
import { CHAT_COLUMN, CHAT_GUTTER } from "./chat-column";

const STARTER_KEYS = ["list_open", "summarize_today", "plan_next"] as const;

/**
 * Agent-page welcome, mapped from official https://github.com/lobehub/lobehub
 * `src/features/AgentHome` (`lobehub/lobe-chat` 301s to that same repository).
 * A flex spacer pins AgentInfo + OpeningQuestions above the composer. Chips
 * call `fillInputMessage` rather than sending. Avatars stay circular
 * (`ActorAvatar`); LobeHub's square welcome avatar is not copied.
 */
export function EmptyState({ agent }: { agent: Agent | null }) {
  const { t } = useT("chat");
  const description = agent?.description?.trim();
  const fillInputMessage = useFillInputMessage();

  return (
    <div className={cn("flex min-h-0 flex-1 flex-col overflow-y-auto", CHAT_GUTTER)}>
      <Flexbox className={cn(CHAT_COLUMN, "min-h-full")} flex={1} width="100%">
        <Flexbox flex={1} />
        <Flexbox gap={32} style={{ paddingBottom: "max(4vh, 16px)" }} width="100%">
          {agent ? (
            <>
              <Flexbox gap={12}>
                <ActorAvatar
                  actorType="agent"
                  actorId={agent.id}
                  size="2xl"
                  className="ring-1 ring-inset ring-border"
                />
                <h3 className="text-display-sm font-bold leading-tight">{agent.name}</h3>
                {description ? (
                  <p className="max-w-[640px] text-body text-muted-foreground">{description}</p>
                ) : null}
              </Flexbox>
              <OpeningQuestions
                questions={STARTER_KEYS.map((key) => t(($) => $.starter_prompts[key]))}
                onPick={fillInputMessage}
              />
            </>
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

function useFillInputMessage() {
  const setInputDraft = useChatStore((s) => s.setInputDraft);
  const activeSessionId = useChatStore((s) => s.activeSessionId);
  return useCallback(
    (text: string) => {
      setInputDraft(activeSessionId ?? DRAFT_NEW_SESSION, text);
    },
    [activeSessionId, setInputDraft],
  );
}

function OpeningQuestions({
  questions,
  onPick,
}: {
  questions: string[];
  onPick: (question: string) => void;
}) {
  const { t } = useT("chat");
  return (
    <div>
      <p className="mb-2 text-body text-muted-foreground">
        {t(($) => $.empty_state.returning_subtitle)}
      </p>
      <Flexbox gap={8} horizontal wrap="wrap">
        {questions.map((question) => (
          <button
            key={question}
            type="button"
            onClick={() => onPick(question)}
            className="rounded-full bg-muted px-4 py-2 text-left text-body text-foreground transition-colors hover:bg-accent"
          >
            {question}
          </button>
        ))}
      </Flexbox>
    </div>
  );
}
