"use client";

import { useEffect, useRef, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { Bot, Gauge, MessageSquare, Server } from "lucide-react";
import type { Agent, AgentRuntime, MemberWithUser } from "@orvilo/core/types";
import type { AgentPresenceDetail } from "@orvilo/core/agents";
import { deviceDisplayName, runtimeModelsOptions } from "@orvilo/core/runtimes";
import { Button } from "@orvilo/ui/components/ui/button";
import { Input } from "@orvilo/ui/components/ui/input";
import { ActorAvatar } from "../../common/actor-avatar";
import { AppLink } from "../../navigation";
import { BreadcrumbHeader } from "../../layout/breadcrumb-header";
import { useT } from "../../i18n";
import { AgentProviderAvatar } from "./agent-provider-avatar";
import { VisibilityBadge } from "./visibility-badge";
import { findModelCapabilityEntry } from "./inspector/model-capability";
import {
  serviceTierDisplayName,
  thinkingLevelDisplayName,
} from "./model-selector-options";

export interface AgentIdentityCardProps {
  /** Render the page identity in the shared shell header instead of a summary card. */
  breadcrumbHref?: string;
  agent: Agent;
  runtime: AgentRuntime | null;
  owner: MemberWithUser | null;
  presence: AgentPresenceDetail | null;
  canEdit: boolean;
  dmPending: boolean;
  dmHref: string;
  onDm: (event: React.MouseEvent<HTMLAnchorElement>) => void;
  onUpdate: (id: string, data: Record<string, unknown>) => Promise<void>;
}

export function AgentIdentityCard({
  breadcrumbHref,
  agent,
  runtime,
  owner,
  presence,
  canEdit,
  dmPending,
  dmHref,
  onDm,
  onUpdate,
}: AgentIdentityCardProps) {
  const { t } = useT("agents");
  const isArchived = !!agent.archived_at;
  const runtimeOnline = runtime?.status === "online";
  const [editing, setEditing] = useState(false);
  const [name, setName] = useState(agent.name);
  const [saving, setSaving] = useState(false);
  const previousAgentIdRef = useRef(agent.id);
  const modelsQuery = useQuery(
    runtimeModelsOptions(!breadcrumbHref && runtimeOnline ? runtime?.id : null, runtime?.workspace_id),
  );
  const catalogEntry = findModelCapabilityEntry(
    modelsQuery.data?.models ?? [],
    agent.model,
    runtime?.provider ?? "",
  );
  const modelSummary = [
    catalogEntry?.label || agent.model || t(($) => $.profile_card.model_unset),
    thinkingLevelDisplayName(agent.thinking_level ?? "", catalogEntry),
    serviceTierDisplayName(
      agent.service_tier ?? "",
      catalogEntry,
      t(($) => $.pickers.service_tier_standard),
    ),
  ]
    .filter(Boolean)
    .join(" · ");

  useEffect(() => {
    if (previousAgentIdRef.current !== agent.id) {
      previousAgentIdRef.current = agent.id;
      setName(agent.name);
      setEditing(false);
    }
  }, [agent.id, agent.name]);

  useEffect(() => {
    if (!editing) setName(agent.name);
  }, [agent.name, editing]);

  const nameInvalid = name.trim().length === 0;

  const handleEditToggle = async () => {
    if (!editing) {
      setName(agent.name);
      setEditing(true);
      return;
    }
    const next = name.trim();
    if (!next) return;
    if (next === agent.name) {
      setEditing(false);
      return;
    }
    setSaving(true);
    try {
      await onUpdate(agent.id, { name: next });
      setEditing(false);
    } catch {
      // The page reports the error; retain the draft for correction or retry.
    } finally {
      setSaving(false);
    }
  };

  const nameField = editing ? (
    <Input
      id="agent-display-name"
      name="agent-name"
      autoComplete="off"
      autoFocus
      aria-label={t(($) => $.inspector.name_label)}
      value={name}
      onChange={(event) => setName(event.target.value)}
      disabled={saving}
      aria-invalid={nameInvalid || undefined}
      className="h-8 min-w-0 flex-1 text-caption"
    />
  ) : (
    <h2
      className="min-w-0 truncate text-body font-medium"
      data-testid="agent-name-value"
      title={agent.name}
    >
      {agent.name}
    </h2>
  );
  const editAction = canEdit ? (
    <button
      type="button"
      disabled={saving || (editing && nameInvalid)}
      onClick={() => void handleEditToggle()}
      aria-label={
        editing
          ? t(($) => $.detail.done_aria)
          : t(($) => $.detail.edit_aria)
      }
      className="shrink-0 text-caption font-medium text-muted-foreground transition-colors hover:text-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring disabled:opacity-50"
    >
      {editing ? t(($) => $.detail.done) : t(($) => $.detail.edit)}
    </button>
  ) : null;
  const messageAction = !isArchived ? (
    <Button
      variant="outline"
      size="sm"
      disabled={dmPending}
      className={`${breadcrumbHref ? "" : "mt-4 "}h-7 px-2 text-caption data-disabled:pointer-events-none data-disabled:opacity-50`}
      render={<AppLink href={dmHref} onClick={onDm} />}
      nativeButton={false}
    >
      <MessageSquare className="size-3.5" aria-hidden="true" />
      {t(($) => $.detail.dm)}
    </Button>
  ) : null;

  if (breadcrumbHref) {
    return (
      <>
        <BreadcrumbHeader
          segments={[{ href: breadcrumbHref, label: t(($) => $.page.title) }]}
          leaf={
            <>
              <AgentProviderAvatar
                provider={runtime?.provider}
                name={agent.name}
                size="sm"
                online={presence?.availability === "online"}
                onlineLabel={t(($) => $.availability.online)}
              />
              {nameField}
            </>
          }
          actions={<>{editAction}{messageAction}</>}
        />
        <div className="flex shrink-0 flex-wrap items-center gap-x-5 gap-y-2 border-b px-4 py-2 text-caption">
          {owner ? (
            <span className="flex min-w-0 items-center gap-2">
              <span className="text-muted-foreground">{t(($) => $.inspector.prop_owner)}</span>
              <ActorAvatar actorType="member" actorId={owner.user_id} size="xs" />
              <span className="truncate">{owner.name}</span>
            </span>
          ) : null}
          <span className="flex items-center gap-2">
            <span className="text-muted-foreground">{t(($) => $.overview.access)}</span>
            <VisibilityBadge value={agent.visibility} />
          </span>
          {editing && nameInvalid ? (
            <span className="text-destructive">{t(($) => $.inspector.rename_required)}</span>
          ) : null}
        </div>
      </>
    );
  }

  return (
    <aside className="w-[320px] self-start rounded-xl border border-surface-border bg-surface p-5 shadow-[var(--surface-shadow)] xl:sticky xl:top-6">
      <div className="flex items-start gap-3">
        <AgentProviderAvatar
          provider={runtime?.provider}
          name={agent.name}
          size="xl"
          online={presence?.availability === "online"}
          onlineLabel={t(($) => $.availability.online)}
        />
        <div className="min-w-0 flex-1">
          <div className="flex min-w-0 items-center gap-2">
            {nameField}
            {editAction}
          </div>
          {editing && nameInvalid ? (
            <p className="mt-1 text-caption text-destructive">
              {t(($) => $.inspector.rename_required)}
            </p>
          ) : null}
        </div>
      </div>

      {messageAction}

      <dl className="mt-5 space-y-3 text-caption">
        {owner ? (
          <SummaryRow label={t(($) => $.inspector.prop_owner)}>
            <span className="flex min-w-0 items-center gap-2 text-foreground">
              <ActorAvatar actorType="member" actorId={owner.user_id} size="xs" />
              <span className="truncate">{owner.name}</span>
            </span>
          </SummaryRow>
        ) : null}
        <SummaryRow label={t(($) => $.overview.access)}>
          <VisibilityBadge
            value={agent.visibility}
            className="gap-2 text-foreground [&_svg]:size-4"
          />
        </SummaryRow>
        <SummaryRow label={t(($) => $.inspector.prop_device)}>
          <SummaryValue
            icon={<Server />}
            value={
              runtime
                ? deviceDisplayName(runtime)
                : t(($) => $.pickers.runtime_none)
            }
          />
        </SummaryRow>
        <SummaryRow label={t(($) => $.inspector.prop_model)}>
          <SummaryValue icon={<Bot />} value={modelSummary} />
        </SummaryRow>
        <SummaryRow label={t(($) => $.inspector.prop_concurrency)}>
          <SummaryValue
            icon={<Gauge />}
            value={String(agent.max_concurrent_tasks)}
            tabular
          />
        </SummaryRow>
      </dl>
    </aside>
  );
}

function SummaryValue({
  icon,
  value,
  tabular = false,
}: {
  icon: React.ReactElement;
  value: string;
  tabular?: boolean;
}) {
  return (
    <span className="flex min-w-0 items-center gap-2 text-foreground [&_svg]:size-4 [&_svg]:shrink-0 [&_svg]:text-muted-foreground">
      {icon}
      <span className={tabular ? "truncate tabular-nums" : "truncate"}>{value}</span>
    </span>
  );
}

function SummaryRow({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <div className="grid grid-cols-[88px_minmax(0,1fr)] items-center gap-3">
      <dt className="text-muted-foreground">{label}</dt>
      <dd className="min-w-0">{children}</dd>
    </div>
  );
}
