"use client";

import { AlertCircle, Check, Plus, Sparkles } from "lucide-react";
import type { AgentAvailability } from "@orvilo/core/agents";
import { isAgentRuntimeBound } from "@orvilo/core/agents";
import { Badge } from "@orvilo/ui/components/reui/badge";
import { Frame, FramePanel } from "@orvilo/ui/components/reui/frame";
import { IconTile } from "@orvilo/ui/components/reui/icon-tile";
import { Button } from "@orvilo/ui/components/ui/button";
import {
  Item,
  ItemActions,
  ItemContent,
  ItemGroup,
  ItemMedia,
  ItemSeparator,
  ItemTitle,
} from "@orvilo/ui/components/ui/item";
import { Skeleton } from "@orvilo/ui/components/ui/skeleton";
import { cn } from "@orvilo/ui/lib/utils";
import { ActorAvatar } from "../../common/actor-avatar";
import { ProviderLogo } from "../../runtimes/components/provider-logo";
import { useT } from "../../i18n";
import { AgentRowActions } from "./agent-row-actions";
import type { AgentListRow } from "./agents-page";
import { availabilityConfig } from "../presence";

/**
 * Grid layout for agent identity cards. The card answers who an agent is and
 * whether it can run: name, description, availability, model, runtime.
 */
export const AGENT_CARD_GRID_CLASS =
  "grid-cols-1 sm:grid-cols-2 xl:grid-cols-3 [@container(min-width:30rem)]:grid-cols-2 [@container(min-width:60rem)]:grid-cols-3";

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
      className="group flex w-full min-w-0 flex-col items-center justify-center gap-3 rounded-xl border border-dashed border-border bg-transparent p-5 text-muted-foreground transition-colors hover:border-primary/50 hover:bg-primary/5 hover:text-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
      data-testid="new-agent-card"
      onClick={onClick}
      type="button"
    >
      <IconTile variant="soft" size="lg" radius="full">
        <Plus />
      </IconTile>
      <span className="text-body font-semibold text-foreground">
        {t(($) => $.page.new_agent)}
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
  const { agent, presence, runtime } = row;

  const availability: AgentAvailability | null = agent.archived_at
    ? "archived"
    : (presence?.availability ?? null);
  const visual =
    availability === null ? null : availabilityConfig[availability];
  const needsRuntime = !agent.archived_at && !isAgentRuntimeBound(agent);

  // The description is what distinguishes one agent from another, so it owns
  // the subtitle. Owner and runtime are properties and live in the list below.
  const subtitle = agent.description || t(($) => $.row.no_description);

  const properties = [
    {
      key: "model",
      label: agent.model || t(($) => $.profile_card.model_unset),
      icon: <Sparkles aria-hidden="true" />,
    },
    {
      key: "runtime",
      label: runtime?.name || t(($) => $.row.needs_runtime),
      icon: runtime ? (
        <ProviderLogo provider={runtime.provider} className="size-full" />
      ) : (
        <AlertCircle aria-hidden="true" />
      ),
    },
  ];

  return (
    <Frame
      spacing="xs"
      className={cn(
        "group/card w-full min-w-0",
        selected && "bg-primary/5 ring-1 ring-primary/20",
      )}
      data-testid={`agent-card-${agent.id}`}
    >
      <FramePanel className="relative isolate px-4 py-4">
        {/* Full-bleed target keeps the whole card clickable and focusable. */}
        <button
          aria-label={t(($) => $.profile_panel.open_profile, {
            name: agent.name,
          })}
          className="absolute inset-0 z-0 rounded-[inherit] focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
          onClick={onOpenSummary}
          type="button"
        />

        <div className="pointer-events-none relative z-10 flex flex-col gap-4">
          {/* Header: identity */}
          <div className="flex items-start gap-3">
            <div className="relative shrink-0">
              <ActorAvatar
                actorId={agent.id}
                actorType="agent"
                className="bg-background ring-1 ring-border"
                profileLink={false}
                size="lg"
              />
              {needsRuntime ? (
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
              <span className="truncate text-body font-semibold leading-tight text-foreground">
                {agent.name}
              </span>
              <span className="line-clamp-2 text-caption text-muted-foreground">
                {subtitle}
              </span>
            </div>

            <div className="pointer-events-auto relative z-20 flex shrink-0 items-center gap-1">
              <Button
                aria-label={
                  selected
                    ? t(($) => $.profile_panel.deselect, { name: agent.name })
                    : t(($) => $.profile_panel.select, { name: agent.name })
                }
                aria-pressed={selected}
                className={cn(
                  "size-7 p-0 text-muted-foreground transition-opacity hover:text-foreground focus-visible:opacity-100",
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
                    selected
                      ? "border-primary bg-primary text-primary-foreground"
                      : "border-input",
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

          {/* Properties */}
          <ItemGroup className="gap-0!">
            {properties.map((property, index) => (
              <div className="contents" key={property.key}>
                <Item size="sm" className="flex-nowrap px-0 py-0">
                  <ItemMedia>
                    <IconTile variant="elevated" size="sm">
                      {property.icon}
                    </IconTile>
                  </ItemMedia>
                  <ItemContent className="min-w-0">
                    <ItemTitle className="w-full min-w-0">
                      <span className="truncate text-caption text-muted-foreground">
                        {property.key === "model" ? (
                          <span className="font-mono">{property.label}</span>
                        ) : (
                          property.label
                        )}
                      </span>
                    </ItemTitle>
                  </ItemContent>
                  {property.key === "runtime" && !runtime ? (
                    <ItemActions className="shrink-0">
                      <Badge variant="warning-light" size="xs">
                        {t(($) => $.row.needs_runtime)}
                      </Badge>
                    </ItemActions>
                  ) : null}
                </Item>
                {index < properties.length - 1 ? (
                  <ItemSeparator className="my-2 border-t border-dashed bg-transparent" />
                ) : null}
              </div>
            ))}
          </ItemGroup>
        </div>
      </FramePanel>
    </Frame>
  );
}

export function AgentCardSkeleton() {
  return (
    <Frame spacing="xs" className="w-full min-w-0">
      <FramePanel className="px-4 py-4">
        <div className="flex flex-col gap-4">
          <div className="flex items-start gap-3">
            <Skeleton className="size-10 rounded-full" />
            <div className="flex flex-1 flex-col gap-1.5">
              <Skeleton className="h-4 w-28 max-w-full" />
              <Skeleton className="h-3 w-40 max-w-full" />
            </div>
            <Skeleton className="size-6 rounded-md" />
          </div>
          <Skeleton className="h-14 w-full rounded-lg" />
          <div className="flex flex-col gap-3">
            <Skeleton className="h-8 w-full" />
            <Skeleton className="h-8 w-full" />
          </div>
        </div>
      </FramePanel>
    </Frame>
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
