"use client";

import { AlertCircle, Check, Plus } from "lucide-react";
import type { AgentAvailability } from "@orvilo/core/agents";
import { isAgentRuntimeBound } from "@orvilo/core/agents";
import { Badge } from "@orvilo/ui/components/reui/badge";
import { Button } from "@orvilo/ui/components/ui/button";
import { Skeleton } from "@orvilo/ui/components/ui/skeleton";
import { cn } from "@orvilo/ui/lib/utils";
import { ActorAvatar } from "../../common/actor-avatar";
import { useT } from "../../i18n";
import { AgentRowActions } from "./agent-row-actions";
import type { AgentListRow } from "./agents-page";
import { availabilityConfig } from "../presence";

/**
 * Grid layout for agent identity cards.
 */
export const AGENT_CARD_GRID_CLASS =
  "grid-cols-1 sm:grid-cols-2 lg:grid-cols-3 xl:grid-cols-4 [@container(min-width:24rem)]:grid-cols-2 [@container(min-width:44rem)]:grid-cols-3 [@container(min-width:64rem)]:grid-cols-4";

export function AgentCreateCard({
  ariaLabel,
  onClick,
}: {
  ariaLabel: string;
  onClick: () => void;
}) {
  const { t } = useT("agents");
  return (
    <button
      aria-label={ariaLabel}
      className="group relative flex min-h-[190px] w-full min-w-0 flex-col items-center justify-center gap-3 overflow-hidden rounded-2xl border-2 border-dashed border-border/70 bg-card/30 p-5 text-muted-foreground shadow-2xs backdrop-blur-xs transition-all duration-200 hover:border-primary/50 hover:bg-primary/5 hover:text-foreground hover:shadow-xs focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
      data-testid="new-agent-card"
      onClick={onClick}
      type="button"
    >
      <div className="flex size-11 items-center justify-center rounded-xl bg-primary/10 text-primary transition-transform duration-200 group-hover:scale-110 group-hover:bg-primary/20">
        <Plus className="size-5" />
      </div>
      <div className="flex flex-col items-center gap-0.5 text-center">
        <span className="text-body font-semibold text-foreground">
          {t(($) => $.page.new_agent)}
        </span>
        <span className="text-caption text-muted-foreground">
          {t(($) => $.profile_panel.title)}
        </span>
      </div>
    </button>
  );
}

export function AgentCard({
  row,
  selected,
  onOpenSummary,
  onToggleSelected,
  duplicateHref,
}: {
  row: AgentListRow;
  selected: boolean;
  onOpenSummary: () => void;
  onToggleSelected: () => void;
  duplicateHref: string;
}) {
  const { t } = useT("agents");
  const { agent, presence, runtime, runCount, owner } = row;

  const availability: AgentAvailability | null = agent.archived_at
    ? "archived"
    : (presence?.availability ?? null);
  const visual =
    availability === null ? null : availabilityConfig[availability];

  return (
    <div
      className={cn(
        "group/card relative flex min-h-[190px] w-full min-w-0 flex-col justify-between overflow-hidden rounded-2xl border p-4.5 text-left shadow-2xs backdrop-blur-xs transition-all duration-200",
        "bg-card/75 hover:border-border hover:bg-card hover:shadow-xs",
        selected
          ? "border-primary/60 bg-primary/5 ring-1 ring-primary/20"
          : "border-border/70",
      )}
      data-testid={`agent-card-${agent.id}`}
    >
      {/* Full-bleed button keeping keyboard accessibility */}
      <button
        aria-label={t(($) => $.profile_panel.open_profile, { name: agent.name })}
        className="absolute inset-0 z-10 rounded-2xl focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
        onClick={onOpenSummary}
        type="button"
      />

      {/* Top Header: Avatar + Title/Owner + Actions */}
      <div className="relative z-20 flex items-start justify-between gap-3">
        <div className="flex items-center gap-3 min-w-0 flex-1">
          <div className="relative shrink-0">
            <ActorAvatar
              actorId={agent.id}
              actorType="agent"
              className="bg-background shadow-2xs ring-1 ring-border/50"
              profileLink={false}
              size="lg"
            />
            {!agent.archived_at && !isAgentRuntimeBound(agent) ? (
              <span
                aria-label={t(($) => $.row.needs_runtime)}
                className="absolute -bottom-0.5 -right-0.5 flex size-3.5 items-center justify-center rounded-full border-2 border-background bg-warning text-warning-foreground"
                role="img"
                title={t(($) => $.row.needs_runtime)}
              >
                <AlertCircle className="size-2" />
              </span>
            ) : availability !== null && visual ? (
              <span
                aria-label={t(($) => $.availability[availability])}
                className={cn(
                  "absolute -bottom-0.5 -right-0.5 size-3.5 rounded-full border-2 border-background",
                  visual.dotClass,
                )}
                role="img"
                title={t(($) => $.availability[availability])}
              />
            ) : null}
          </div>

          <div className="flex min-w-0 flex-1 flex-col gap-0.5">
            <span className="truncate text-body font-semibold text-foreground leading-snug">
              {agent.name}
            </span>
            <span className="truncate text-caption text-muted-foreground">
              {owner?.name || owner?.email || agent.description || t(($) => $.profile_card.model_unset)}
            </span>
          </div>
        </div>

        {/* Selection checkbox + overflow menu */}
        <div className="relative z-30 flex items-center gap-1">
          <Button
            aria-label={
              selected
                ? t(($) => $.profile_panel.deselect, { name: agent.name })
                : t(($) => $.profile_panel.select, { name: agent.name })
            }
            aria-pressed={selected}
            className={cn(
              "size-7 rounded-md bg-background/80 p-0 text-muted-foreground shadow-2xs backdrop-blur-sm transition-opacity hover:bg-background hover:text-foreground focus-visible:opacity-100",
              selected
                ? "opacity-100"
                : "opacity-0 group-hover/card:opacity-100",
              "[@media(hover:none)]:opacity-100",
            )}
            onClick={(event) => {
              event.stopPropagation();
              onToggleSelected();
            }}
            size="icon-sm"
            type="button"
            variant="ghost"
          >
            <span
              aria-hidden="true"
              className={cn(
                "pointer-events-none flex size-3.5 items-center justify-center rounded-sm border",
                selected ? "border-primary bg-primary text-primary-foreground" : "border-input",
              )}
            >
              {selected ? <Check className="size-2.5" /> : null}
            </span>
          </Button>

          <AgentRowActions
            agent={agent}
            alwaysVisible
            canManage={row.canManage}
            duplicateHref={duplicateHref}
            iconVariant="vertical"
            presence={row.presence}
          />
        </div>
      </div>

      {/* Middle: Badges (Model & Runtime) */}
      <div className="relative z-20 flex flex-wrap items-center gap-1.5 my-2.5">
        {agent.model ? (
          <Badge
            variant="outline"
            className="border-border/60 bg-muted/40 font-mono text-micro text-foreground"
          >
            {agent.model}
          </Badge>
        ) : (
          <Badge
            variant="outline"
            className="border-border/50 text-micro text-muted-foreground"
          >
            {t(($) => $.profile_card.model_unset)}
          </Badge>
        )}

        {runtime ? (
          <Badge
            variant="outline"
            className="border-border/50 text-micro text-muted-foreground"
          >
            {runtime.name}
          </Badge>
        ) : (
          <Badge
            variant="outline"
            className="border-warning/40 text-micro text-warning"
          >
            {t(($) => $.row.needs_runtime)}
          </Badge>
        )}
      </div>

      {/* Bottom: Activity / Run Count + Status badge */}
      <div className="relative z-20 flex items-center justify-between border-t border-border/50 pt-2.5 text-caption text-muted-foreground">
        <span className="tabular-nums">
          {runCount > 0
            ? t(($) => $.kanban.runs_count, { count: runCount })
            : t(($) => $.kanban.no_runs)}
        </span>

        {availability && visual ? (
          <span className="inline-flex items-center gap-1.5 font-medium text-micro">
            <span className={cn("size-1.5 rounded-full", visual.dotClass)} />
            {t(($) => $.availability[availability])}
          </span>
        ) : null}
      </div>
    </div>
  );
}

export function AgentCardSkeleton() {
  return (
    <div className="relative flex min-h-[190px] w-full min-w-0 flex-col justify-between overflow-hidden rounded-2xl border border-border/70 bg-card/60 p-4.5 shadow-xs">
      <div className="flex items-start justify-between gap-3">
        <div className="flex items-center gap-3">
          <Skeleton className="size-10 rounded-full" />
          <div className="flex flex-col gap-1.5">
            <Skeleton className="h-4 w-28 max-w-full" />
            <Skeleton className="h-3 w-20 max-w-full" />
          </div>
        </div>
        <Skeleton className="size-6 rounded-md" />
      </div>
      <div className="flex gap-2">
        <Skeleton className="h-5 w-20 rounded-md" />
        <Skeleton className="h-5 w-16 rounded-md" />
      </div>
      <div className="flex items-center justify-between border-t border-border/50 pt-2.5">
        <Skeleton className="h-3.5 w-16" />
        <Skeleton className="h-3.5 w-20" />
      </div>
    </div>
  );
}

export function AgentCardsLoadingSkeleton() {
  return (
    <div className={`grid gap-4 ${AGENT_CARD_GRID_CLASS}`}>
      {Array.from({ length: 6 }).map((_, index) => (
        <AgentCardSkeleton key={index} />
      ))}
    </div>
  );
}
