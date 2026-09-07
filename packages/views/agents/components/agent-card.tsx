"use client";

import { AlertCircle, Plus } from "lucide-react";
import type { AgentAvailability } from "@patchbay/core/agents";
import { isAgentRuntimeBound } from "@patchbay/core/agents";
import { Button } from "@patchbay/ui/components/ui/button";
import { Checkbox } from "@patchbay/ui/components/ui/checkbox";
import { Skeleton } from "@patchbay/ui/components/ui/skeleton";
import { cn } from "@patchbay/ui/lib/utils";
import { ActorAvatar } from "../../common/actor-avatar";
import { useT } from "../../i18n";
import { AgentRowActions } from "./agent-row-actions";
import type { AgentListRow } from "./agents-page";
import { availabilityConfig } from "../presence";

/**
 * The card proportions and container breakpoints mirror Buzz's identity-card
 * gallery. The grid is container-query driven so the same gallery remains
 * useful when the profile panel is open beside it.
 */
export const AGENT_CARD_GRID_CLASS =
  "grid-cols-1 [@container(min-width:21rem)]:grid-cols-2 [@container(min-width:32rem)]:grid-cols-3 [@container(min-width:43rem)]:grid-cols-4 [@container(min-width:54rem)]:grid-cols-5";

interface AgentIdentityCardProps {
  actions?: React.ReactNode;
  ariaLabel: string;
  avatar?: React.ReactNode;
  dataTestId: string;
  label: string;
  selected?: boolean;
  subtitle?: string | null;
  onClick: () => void;
}

/**
 * Buzz's card interaction model uses a real full-bleed button behind the
 * visual card. That keeps the whole surface keyboard-accessible while leaving
 * the action menu and selection control as independent siblings.
 */
export function AgentIdentityCard({
  actions,
  ariaLabel,
  avatar,
  dataTestId,
  label,
  selected = false,
  subtitle,
  onClick,
}: AgentIdentityCardProps) {
  return (
    <div
      className={cn(
        "group/identity relative aspect-[4/5] w-full min-w-0 overflow-hidden rounded-2xl border bg-muted/50 text-left shadow-xs transition-colors hover:border-border hover:bg-muted/65",
        selected
          ? "border-brand/60 bg-brand/7 ring-1 ring-brand/20"
          : "border-border/70",
      )}
      data-testid={dataTestId}
    >
      <button
        aria-label={ariaLabel}
        className="absolute inset-0 z-10 rounded-2xl focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
        onClick={onClick}
        type="button"
      />

      <div className="pointer-events-none relative z-20 flex h-full w-full min-w-0 flex-col items-center justify-center gap-5 px-4 pb-12 text-center">
        <div className="flex h-24 w-24 items-center justify-center">
          {avatar}
        </div>
      </div>

      {actions ? (
        <div className="absolute top-3 right-3 z-40 flex items-center gap-1">
          {actions}
        </div>
      ) : null}

      <div className="pointer-events-none absolute right-3 bottom-3 left-3 z-30 flex min-w-0 items-end gap-2 text-left leading-5">
        <div className="flex min-w-0 flex-1 flex-col gap-0.5">
          <span className="min-w-0 truncate text-body font-semibold tracking-normal text-foreground">
            {label}
          </span>
          {subtitle ? (
            <span className="line-clamp-2 min-w-0 text-caption font-normal text-muted-foreground">
              {subtitle}
            </span>
          ) : null}
        </div>
      </div>
    </div>
  );
}

export function AgentCreateCard({
  ariaLabel,
  onClick,
}: {
  ariaLabel: string;
  onClick: () => void;
}) {
  return (
    <button
      aria-label={ariaLabel}
      className="group relative flex aspect-[4/5] w-full min-w-0 items-center justify-center overflow-hidden rounded-2xl border border-dashed border-border/80 bg-transparent text-muted-foreground shadow-xs transition-colors hover:border-border hover:bg-muted/70 hover:text-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
      data-testid="new-agent-card"
      onClick={onClick}
      type="button"
    >
      <span className="flex flex-col items-center justify-center gap-2 text-center">
        <Plus className="size-7 transition-transform group-hover:scale-105" />
      </span>
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
  const { agent } = row;
  const subtitle =
    agent.description.trim() ||
    agent.model.trim() ||
    t(($) => $.profile_card.model_unset);

  return (
    <AgentIdentityCard
      actions={
        <>
          <Button
            aria-label={
              selected
                ? t(($) => $.profile_panel.deselect, { name: agent.name })
                : t(($) => $.profile_panel.select, { name: agent.name })
            }
            aria-pressed={selected}
            className={cn(
              "size-7 rounded-md bg-background/80 p-0 text-muted-foreground shadow-xs backdrop-blur-sm transition-opacity hover:bg-background hover:text-foreground focus-visible:opacity-100",
              selected
                ? "opacity-100"
                : "opacity-0 group-hover/identity:opacity-100",
            )}
            onClick={(event) => {
              event.stopPropagation();
              onToggleSelected();
            }}
            size="icon-sm"
            type="button"
            variant="ghost"
          >
            <Checkbox
              checked={selected}
              className="pointer-events-none"
              tabIndex={-1}
            />
          </Button>
          <AgentRowActions
            agent={agent}
            alwaysVisible
            canManage={row.canManage}
            duplicateHref={duplicateHref}
            iconVariant="vertical"
            presence={row.presence}
          />
        </>
      }
      ariaLabel={`${agent.name} agent profile`}
      avatar={<AgentCardAvatar row={row} />}
      dataTestId={`agent-card-${agent.id}`}
      label={agent.name}
      selected={selected}
      subtitle={subtitle}
      onClick={onOpenSummary}
    />
  );
}

function AgentCardAvatar({ row }: { row: AgentListRow }) {
  const { t } = useT("agents");
  const { agent, presence } = row;
  const availability: AgentAvailability | null = agent.archived_at
    ? "archived"
    : (presence?.availability ?? null);
  const visual =
    availability === null ? null : availabilityConfig[availability];

  return (
    <div className="relative flex h-24 w-24 items-center justify-center">
      <ActorAvatar
        actorId={agent.id}
        actorType="agent"
        className="scale-[1.7] bg-background ring-2 ring-background shadow-sm"
        profileLink={false}
        size="2xl"
      />
      {!agent.archived_at && !isAgentRuntimeBound(agent) ? (
        <span
          aria-label={t(($) => $.row.needs_runtime)}
          className="absolute right-1 bottom-1 flex size-4 items-center justify-center rounded-full border-2 border-background bg-warning text-warning-foreground"
          role="img"
          title={t(($) => $.row.needs_runtime)}
        >
          <AlertCircle className="size-2.5" />
        </span>
      ) : availability !== null && visual ? (
        <span
          aria-label={t(($) => $.availability[availability])}
          className={cn(
            "absolute right-1 bottom-1 size-4 rounded-full border-2 border-background",
            visual.dotClass,
          )}
          role="img"
          title={t(($) => $.availability[availability])}
        />
      ) : null}
    </div>
  );
}

export function AgentCardSkeleton() {
  return (
    <div className="relative aspect-[4/5] w-full min-w-0 overflow-hidden rounded-2xl border border-border/70 bg-muted/50 shadow-xs">
      <div className="absolute inset-x-0 top-0 bottom-12 flex items-center justify-center">
        <Skeleton className="size-24 rounded-full" />
      </div>
      <div className="absolute right-3 bottom-3 left-3 flex min-w-0 flex-col gap-1">
        <Skeleton className="h-4 w-28 max-w-full" />
        <Skeleton className="h-3.5 w-20 max-w-full" />
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
