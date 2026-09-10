"use client";

import { useQuery } from "@tanstack/react-query";
import type { Agent, AgentRuntime } from "@orvilo/core/types";
import { useAgentPresenceDetail } from "@orvilo/core/agents";
import { useWorkspaceId } from "@orvilo/core/hooks";
import {
  deriveRuntimeHealth,
  runtimeDisplayLabel,
  type RuntimeHealth,
} from "@orvilo/core/runtimes";
import { agentListOptions, memberListOptions } from "@orvilo/core/workspace/queries";
import { runtimeListOptions } from "@orvilo/core/runtimes/queries";
import { useWorkspacePaths } from "@orvilo/core/paths";
import { AgentIdentityAvatar } from "../../common/agent-identity-avatar";
import { Skeleton } from "@orvilo/ui/components/ui/skeleton";
import { AppLink } from "../../navigation";
import { HealthIcon } from "../../runtimes/components/shared";
import { availabilityConfig } from "../presence";
import { VisibilityBadge } from "./visibility-badge";
import { useT } from "../../i18n";

interface AgentProfileCardProps {
  agentId: string;
}

export function AgentProfileCard({ agentId }: AgentProfileCardProps) {
  const { t } = useT("agents");
  const wsId = useWorkspaceId();
  const p = useWorkspacePaths();
  const { data: agents = [], isLoading: agentsLoading } = useQuery(agentListOptions(wsId));
  const { data: members = [] } = useQuery(memberListOptions(wsId));
  const { data: runtimes = [] } = useQuery(runtimeListOptions(wsId));

  const agent = agents.find((a) => a.id === agentId);

  if (agentsLoading && !agent) {
    return (
      <div className="flex items-center gap-3">
        <Skeleton className="h-10 w-10 rounded-full" />
        <div className="flex-1 space-y-1.5">
          <Skeleton className="h-4 w-28" />
          <Skeleton className="h-3 w-20" />
        </div>
      </div>
    );
  }

  if (!agent) {
    return (
      <div className="text-caption text-muted-foreground">{t(($) => $.profile_card.unavailable)}</div>
    );
  }

  const owner = agent.owner_id
    ? members.find((m) => m.user_id === agent.owner_id) ?? null
    : null;
  const runtime = runtimes.find((r) => r.id === agent.runtime_id) ?? null;
  const isArchived = !!agent.archived_at;

  return (
    // `group` enables the hover-only Detail link on the top-right —
    // it fades in only when the user is hovering the card chrome,
    // staying out of the way during a quick glance.
    <div className="group flex flex-col gap-3 text-left">
      {/* Header — avatar + name + availability on the left, "Detail →" link
          on the right (hover-only). Card stays minimal: only the 3-state
          availability dot is surfaced here; last-task state lives in the
          agents list and the agent detail page. */}
      <div className="flex items-start gap-3">
        {/* Base avatar rather than the ActorAvatar wrapper: this card IS a
            hover-card payload, so it must not nest another hover card or
            profile link inside itself. */}
        <AgentIdentityAvatar agentId={agent.id} name={agent.name} size="xl" />
        <div className="min-w-0 flex-1">
          <div className="flex items-center gap-1.5">
            <p className="truncate text-body font-semibold">{agent.name}</p>
            {!isArchived && <VisibilityBadge value={agent.visibility} compact />}
            {isArchived && (
              <span className="rounded-md bg-muted px-1.5 py-0.5 text-micro font-medium text-muted-foreground">
                {t(($) => $.row.archived)}
              </span>
            )}
          </div>
          {!isArchived && (
            <AgentAvailabilityLine wsId={wsId} agentId={agent.id} />
          )}
        </div>
        {!isArchived && (
          <AppLink
            href={p.agentDetail(agent.id)}
            className="mr-1 mt-0.5 shrink-0 text-caption font-normal text-brand opacity-0 transition-opacity group-hover:opacity-100"
          >
            {t(($) => $.profile_card.detail_link)}
          </AppLink>
        )}
      </div>

      {/* Meta rows — runtime (where it lives), model (what powers it),
          owner (who manages it). Skills are workspace-shared in Settings. */}
      <div className="flex flex-col gap-1.5 text-caption">
        <RuntimeRow agent={agent} runtime={runtime} />
        <ModelRow model={agent.model} thinkingLevel={agent.thinking_level} />
        {owner && <MetaRow label={t(($) => $.profile_card.owner_label)} value={owner.name} />}
      </div>
    </div>
  );
}

// Compact availability line under the agent name — single 3-state signal
// (online / unstable / offline). Last-task state is intentionally NOT
// shown here; it belongs in the agents list and the detail page where
// there's room for icon + label + reason without crowding the popover.
function AgentAvailabilityLine({
  wsId,
  agentId,
}: {
  wsId: string | undefined;
  agentId: string;
}) {
  const { t } = useT("agents");
  const detail = useAgentPresenceDetail(wsId, agentId);
  if (detail === "loading") {
    return <Skeleton className="mt-0.5 h-3 w-16" />;
  }
  const av = availabilityConfig[detail.availability];
  return (
    <div className="mt-0.5 inline-flex items-center gap-1.5">
      <span className={`h-1.5 w-1.5 rounded-full ${av.dotClass}`} />
      <span className={`text-caption ${av.textClass}`}>{t(($) => $.availability[detail.availability])}</span>
    </div>
  );
}

// Compact runtime row — wifi-style health icon + runtime name. The icon
// shape (Wifi / WifiOff) plus colour reflects the live runtime health
// derived from runtime + clock; cloud runtimes always read as online.
// This is duplicate signal with the availability dot above by design —
// the dot is the agent's effective availability (which mostly tracks
// runtime health), and seeing the same wifi icon next to the runtime
// name confirms WHICH runtime is the one currently in the dot's state.
function RuntimeRow({
  agent,
  runtime,
}: {
  agent: Agent;
  runtime: AgentRuntime | null;
}) {
  const { t } = useT("agents");
  const isCloud = agent.runtime_mode === "cloud";
  const health: RuntimeHealth = isCloud
    ? "online"
    : runtime
      ? deriveRuntimeHealth(runtime, Date.now())
      : "offline";
  const label = runtime
    ? runtimeDisplayLabel(runtime)
    : isCloud
      ? t(($) => $.row.fallback_runtime_cloud)
      : t(($) => $.profile_card.unknown_runtime);
  return (
    <div className="flex items-center gap-1.5">
      <span className="w-12 shrink-0 text-muted-foreground">{t(($) => $.profile_card.runtime_label)}</span>
      <HealthIcon health={health} className="h-3 w-3 shrink-0" />
      <span className="min-w-0 truncate" title={label}>
        {label}
      </span>
    </div>
  );
}

// Model row — the runtime-native model id the agent runs (`agent.model`),
// with the reasoning/effort token appended as a small badge whenever the
// agent pins one. An empty model means the runtime manages the model, so
// we render that rather than a bare dash. The model id is mono so provider-slug ids like
// `claude-opus-4-8` stay legible. The effort badge is gated on `effort`
// alone (NOT on `hasModel`): an agent whose model is runtime-managed can
// still persist a `thinking_level` that applies at run time.
function ModelRow({
  model,
  thinkingLevel,
}: {
  model: string;
  thinkingLevel?: string;
}) {
  const { t } = useT("agents");
  const hasModel = model.trim().length > 0;
  const effort = thinkingLevel?.trim();
  return (
    <div className="flex items-center gap-1.5">
      <span className="w-12 shrink-0 text-muted-foreground">
        {t(($) => $.profile_card.model_label)}
      </span>
      {hasModel ? (
        <span className="min-w-0 truncate font-mono text-micro" title={model}>
          {model}
        </span>
      ) : (
        <span className="min-w-0 truncate text-muted-foreground">
          {t(($) => $.profile_card.model_unset)}
        </span>
      )}
      {effort && (
        <span className="shrink-0 rounded-md bg-muted px-1.5 py-0.5 text-micro font-medium text-muted-foreground">
          {effort}
        </span>
      )}
    </div>
  );
}

function MetaRow({
  label,
  value,
  mono,
}: {
  label: string;
  value: string;
  mono?: boolean;
}) {
  return (
    <div className="flex items-center gap-1.5">
      <span className="w-12 shrink-0 text-muted-foreground">{label}</span>
      <span className={`truncate ${mono ? "font-mono text-micro" : ""}`} title={value}>
        {value}
      </span>
    </div>
  );
}
