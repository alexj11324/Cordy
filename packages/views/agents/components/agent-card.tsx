"use client";

import { Bot, Monitor, Plus, UserRound } from "lucide-react";
import {
  effectiveAccessScope,
  isAgentRuntimeBound,
  type AgentAvailability,
} from "@orvilo/core/agents";
import {
  deviceKind,
  deviceLabelForViewer,
} from "@orvilo/core/runtimes";
import { resolvePublicFileUrl } from "@orvilo/core/workspace/avatar-url";
import {
  Card,
  CardContent,
  CardHeader,
} from "@orvilo/ui/components/reui/card";
import { Skeleton } from "@orvilo/ui/components/ui/skeleton";
import { ProviderLogo } from "../../runtimes/components/provider-logo";
import { useT } from "../../i18n";
import type { AgentListRow } from "./agents-page";
import { DeviceKindIcon } from "./device-kind-icon";
import {
  AtlasDealCard,
  ATLAS_DEAL_CARD_SIZE_CLASS,
  type AtlasDealCardOpportunity,
} from "./atlas-deal-card";

/**
 * Atlas DealCard lives in an 18.5rem kanban column. Keep that width so the
 * snapshot boxes, progress track, and type scale match the template. Do not
 * stretch with `grid-cols-3` / `1fr`.
 */
export const AGENT_CARD_GRID_CLASS =
  "justify-start [grid-template-columns:repeat(auto-fill,minmax(min(100%,18.5rem),18.5rem))]";

export const AGENT_CARD_GRID_GAP_CLASS = "gap-3";

export function AgentCreateCard({
  ariaLabel,
  onClick,
}: {
  ariaLabel: string;
  onClick: () => void;
}) {
  const { t } = useT("agents");
  return (
    <Card
      className={`${ATLAS_DEAL_CARD_SIZE_CLASS} w-full min-w-0 gap-0 border border-dashed border-border/70 bg-card p-0 shadow-xs`}
      size="sm"
    >
      <button
        aria-label={ariaLabel}
        className="flex h-full w-full min-w-0 flex-col items-center justify-center gap-2 rounded-[inherit] px-4 py-5 text-body text-muted-foreground transition-colors hover:bg-primary/5 hover:text-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
        data-testid="new-agent-card"
        onClick={onClick}
        type="button"
      >
        <Plus aria-hidden="true" className="size-5" />
        <span className="font-medium text-foreground">
          {t(($) => $.page.new_agent)}
        </span>
      </button>
    </Card>
  );
}

export function AgentCard({
  row,
  localDaemonId,
  localMachineName,
  currentUserId,
  onOpenSummary,
}: {
  row: AgentListRow;
  localDaemonId?: string | null;
  localMachineName?: string | null;
  currentUserId?: string | null;
  onOpenSummary: () => void;
}) {
  const { t } = useT("agents");
  return (
    <AtlasDealCard
      onOpen={onOpenSummary}
      opportunity={toAtlasDealCard(row, t, {
        localDaemonId,
        localMachineName,
        currentUserId,
      })}
    />
  );
}

function toAtlasDealCard(
  row: AgentListRow,
  t: ReturnType<typeof useT<"agents">>["t"],
  localMachine: {
    localDaemonId?: string | null;
    localMachineName?: string | null;
    currentUserId?: string | null;
  },
): AtlasDealCardOpportunity {
  const { agent, presence, runtime, owner } = row;
  const needsRuntime = !agent.archived_at && !isAgentRuntimeBound(agent);
  const availability = getCardAvailability(row);
  const status = {
    label:
      availability === "online"
        ? t(($) => $.gallery_card.status_online)
        : availability === "unstable"
          ? t(($) => $.gallery_card.status_unstable)
          : availability === "archived"
            ? t(($) => $.gallery_card.status_archived)
            : t(($) => $.gallery_card.status_offline),
    variant:
      availability === "online"
        ? "success-light"
        : availability === "unstable"
          ? "warning-light"
          : availability === "offline"
            ? "destructive-light"
            : "secondary",
    dotClass:
      availability === "online"
        ? "bg-success"
        : availability === "unstable"
          ? "bg-warning"
          : availability === "offline"
            ? "bg-destructive"
            : "bg-muted-foreground/50",
  } satisfies AtlasDealCardOpportunity["status"];

  const access = effectiveAccessScope(
    agent.permission_mode,
    agent.invocation_targets,
  );
  const accessLabel =
    access === "workspace"
      ? t(($) => $.access.scope_labels.workspace)
      : access === "specific-people"
        ? t(($) => $.access.scope_labels.specific_people)
        : t(($) => $.access.scope_labels.owner_only);

  const deviceLabel = runtime
    ? deviceLabelForViewer(
        runtime,
        localMachine,
        t(($) => $.gallery_card.device_this_machine),
      )
    : t(($) => $.row.needs_device);

  const capacity = Math.max(
    1,
    presence?.capacity ?? agent.max_concurrent_tasks ?? 1,
  );
  const runningCount = Math.max(0, presence?.runningCount ?? 0);
  const availableCount = Math.max(0, capacity - runningCount);
  const availablePercent =
    capacity > 0 ? (availableCount / capacity) * 100 : 100;
  // Base UI renders a zero-valued indicator with no visible fill. Keep a 2%
  // red warning sliver when the agent is fully occupied so the exhausted
  // state remains visible on the card.
  const concurrencyPercent = Math.max(2, availablePercent);
  const concurrencyProgressClass =
    availablePercent <= 0
      ? "**:data-[slot=progress-indicator]:bg-destructive"
      : availablePercent <= 50
        ? "**:data-[slot=progress-indicator]:bg-warning"
        : "**:data-[slot=progress-indicator]:bg-success";

  const ownerName = owner?.name ?? agent.owner_id?.slice(0, 8) ?? "";
  const ownerAvatarSrc =
    (owner?.avatar_url ? resolvePublicFileUrl(owner.avatar_url) : null) ?? "";

  return {
    id: agent.id,
    account: agent.name,
    status,
    logo:
      runtime && !needsRuntime ? (
        <ProviderLogo className="size-5" provider={runtime.provider} />
      ) : (
        <Bot aria-hidden="true" className="size-5" />
      ),
    accessIcon: <UserRound aria-hidden="true" className="size-3.5" />,
    accessLabel: t(($) => $.gallery_card.access),
    accessText: accessLabel,
    deviceIcon: runtime ? (
      <DeviceKindIcon kind={deviceKind(runtime)} className="size-3.5" />
    ) : (
      <Monitor aria-hidden="true" className="size-3.5" />
    ),
    deviceLabel: t(($) => $.gallery_card.device),
    deviceText: deviceLabel,
    concurrencyLabel: t(($) => $.gallery_card.concurrency),
    concurrencyText: t(($) => $.gallery_card.concurrency_available_ratio, {
      available: availableCount,
      capacity,
    }),
    concurrency: concurrencyPercent,
    concurrencyProgressClass,
    concurrencyCaption: t(($) => $.gallery_card.concurrency_label, {
      running: runningCount,
      capacity,
      available: availableCount,
      name: agent.name,
    }),
    owner: {
      id: owner?.user_id || agent.owner_id || "",
      name: ownerName,
      avatar: ownerAvatarSrc,
    },
  };
}

function getCardAvailability(row: AgentListRow): AgentAvailability {
  if (row.agent.archived_at) return "archived";
  if (!isAgentRuntimeBound(row.agent) || !row.runtime) return "offline";
  return (
    row.presence?.availability ??
    (row.runtime.status === "online" ? "online" : "offline")
  );
}

export function AgentCardSkeleton() {
  return (
    <Card
      className={`${ATLAS_DEAL_CARD_SIZE_CLASS} w-full min-w-0 gap-0 bg-card p-0 shadow-xs`}
      size="sm"
    >
      <CardHeader className="grid min-h-5 grid-cols-[1.5rem_minmax(0,1fr)] items-center gap-x-2.5 px-3 pt-3 pb-0">
        <Skeleton className="size-5 rounded-md" />
        <Skeleton className="h-4 w-28 max-w-full" />
      </CardHeader>
      <CardContent className="flex min-h-[10.75rem] flex-col gap-3 px-3 pt-3 pb-3">
        <div className="grid grid-cols-2 gap-2">
          <Skeleton className="h-14 w-full rounded-lg" />
          <Skeleton className="h-14 w-full rounded-lg" />
        </div>
        <Skeleton className="h-6 w-full" />
        <Skeleton className="h-7 w-24" />
      </CardContent>
    </Card>
  );
}

export function AgentCardsLoadingSkeleton() {
  return (
    <div className={`grid ${AGENT_CARD_GRID_GAP_CLASS} ${AGENT_CARD_GRID_CLASS}`}>
      {Array.from({ length: 6 }).map((_, index) => (
        <AgentCardSkeleton key={index} />
      ))}
    </div>
  );
}
