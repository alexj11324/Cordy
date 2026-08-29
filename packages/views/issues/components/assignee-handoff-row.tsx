"use client";

import { useMemo } from "react";
import { ArrowRight } from "lucide-react";
import { AVATAR_SIZE_PX, type AvatarSize } from "@patchbay/ui/lib/avatar-size";
import {
  Popover,
  PopoverContent,
  PopoverHeader,
  PopoverTitle,
  PopoverTrigger,
} from "@patchbay/ui/components/ui/popover";
import { cn } from "@patchbay/ui/lib/utils";
import { useActorName } from "@patchbay/core/workspace/hooks";
import type { Issue, TimelineEntry, UpdateIssueRequest } from "@patchbay/core/types";
import { ActorAvatar } from "../../common/actor-avatar";
import { useT } from "../../i18n";
import {
  handoffHopsForDisplay,
  handoffStackActors,
  issueActor,
  reviewHandoffHops,
  type HandoffActor,
  type HandoffHop,
} from "../handoff-chain";
import { AssigneePicker } from "./pickers/assignee-picker";

function HandoffAvatarStack({
  actors,
  size = "sm",
  max = 3,
}: {
  actors: readonly HandoffActor[];
  size?: AvatarSize;
  max?: number;
}) {
  const visible = actors.slice(0, max);
  const overflow = actors.length - visible.length;
  const px = AVATAR_SIZE_PX[size];
  const overlap = Math.round(px * 0.3);

  return (
    <span className="inline-flex items-center">
      {visible.map((actor, i) => (
        <span
          key={`${actor.type}:${actor.id}`}
          style={{ marginLeft: i === 0 ? 0 : -overlap }}
          className="inline-flex rounded-full ring-2 ring-background"
        >
          <ActorAvatar
            actorType={actor.type}
            actorId={actor.id}
            size={size}
            profileLink={false}
          />
        </span>
      ))}
      {overflow > 0 && (
        <span
          style={{
            marginLeft: -overlap,
            width: px,
            height: px,
            fontSize: Math.max(9, Math.round(px * 0.45)),
          }}
          className="inline-flex items-center justify-center rounded-full bg-muted font-medium tabular-nums text-muted-foreground ring-2 ring-background"
        >
          +{overflow}
        </span>
      )}
    </span>
  );
}

function HandoffHistoryPopover({
  hops,
  actors,
}: {
  hops: readonly HandoffHop[];
  actors: readonly HandoffActor[];
}) {
  const { t } = useT("issues");
  const { getActorName } = useActorName();
  const label = t(($) => $.detail.handoff_stack_aria, { count: actors.length });

  return (
    <Popover>
      <PopoverTrigger
        className={cn(
          "inline-flex shrink-0 items-center rounded px-0.5 -mx-0.5",
          "hover:bg-accent/30 transition-colors",
        )}
        aria-label={label}
      >
        <HandoffAvatarStack actors={actors} />
      </PopoverTrigger>
      <PopoverContent align="start" className="w-72 gap-2 p-2.5">
        <PopoverHeader>
          <PopoverTitle className="text-caption font-medium text-muted-foreground">
            {t(($) => $.detail.handoff_history)}
          </PopoverTitle>
        </PopoverHeader>
        <div className="flex flex-col gap-1.5">
          {hops.map((hop, index) => (
            <div
              key={`${hop.from.type}:${hop.from.id}->${hop.to.type}:${hop.to.id}:${index}`}
              className="flex min-w-0 items-center gap-1.5 text-caption"
            >
              <ActorAvatar
                actorType={hop.from.type}
                actorId={hop.from.id}
                size="sm"
                profileLink={false}
              />
              <span className="min-w-0 truncate font-medium">
                {getActorName(hop.from.type, hop.from.id)}
              </span>
              <ArrowRight
                className="size-3.5 shrink-0 text-muted-foreground"
                aria-hidden="true"
              />
              <ActorAvatar
                actorType={hop.to.type}
                actorId={hop.to.id}
                size="sm"
                profileLink={false}
              />
              <span className="min-w-0 truncate font-medium">
                {getActorName(hop.to.type, hop.to.id)}
              </span>
            </div>
          ))}
        </div>
      </PopoverContent>
    </Popover>
  );
}

export function AssigneeHandoffRow({
  issue,
  timeline,
  onUpdate,
}: {
  issue: Issue;
  timeline: readonly TimelineEntry[];
  onUpdate: (updates: Partial<UpdateIssueRequest>) => void;
}) {
  const { getActorName } = useActorName();
  const { actors, hops } = useMemo(() => {
    const recorded = reviewHandoffHops(timeline);
    const assignee = issueActor(issue.assignee_type, issue.assignee_id);
    const reviewer = issueActor(issue.reviewer_type, issue.reviewer_id);
    return {
      actors: handoffStackActors(recorded, assignee, reviewer),
      hops: handoffHopsForDisplay(recorded, assignee, reviewer),
    };
  }, [
    timeline,
    issue.assignee_type,
    issue.assignee_id,
    issue.reviewer_type,
    issue.reviewer_id,
  ]);

  const stacked = actors.length > 1 && hops.length > 0;
  const picker = (
    <AssigneePicker
      assigneeType={issue.assignee_type}
      assigneeId={issue.assignee_id}
      onUpdate={onUpdate}
      align="start"
      trigger={
        stacked && issue.assignee_type && issue.assignee_id ? (
          <span className="truncate">
            {getActorName(issue.assignee_type, issue.assignee_id)}
          </span>
        ) : undefined
      }
    />
  );

  if (!stacked) return picker;

  return (
    <div className="flex min-w-0 items-center gap-1.5">
      <HandoffHistoryPopover hops={hops} actors={actors} />
      {picker}
    </div>
  );
}
