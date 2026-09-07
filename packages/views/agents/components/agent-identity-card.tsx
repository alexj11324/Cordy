"use client";

import { useEffect, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import {
  Bot,
  MessageSquare,
  MoreHorizontal,
  Plus,
  Server,
  Trash2,
} from "lucide-react";
import type {
  Agent,
  AgentRuntime,
  MemberWithUser,
} from "@patchbay/core/types";
import type { AgentPresenceDetail } from "@patchbay/core/agents";
import {
  runtimeDisplayLabel,
  runtimeModelsOptions,
} from "@patchbay/core/runtimes";
import { Button } from "@patchbay/ui/components/ui/button";
import { Input } from "@patchbay/ui/components/ui/input";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@patchbay/ui/components/ui/dropdown-menu";
import { ActorAvatar } from "../../common/actor-avatar";
import { AvatarUploadControl } from "../../common/avatar-upload-control";
import { AppLink } from "../../navigation";
import { useT } from "../../i18n";
import { AgentPresenceIndicator } from "./agent-presence-indicator";
import { VisibilityBadge } from "./visibility-badge";
import { findModelCapabilityEntry } from "./inspector/model-capability";
import {
  serviceTierDisplayName,
  thinkingLevelDisplayName,
} from "./model-selector-options";

export interface AgentIdentityCardProps {
  agent: Agent;
  runtime: AgentRuntime | null;
  owner: MemberWithUser | null;
  presence: AgentPresenceDetail | null;
  canAssign: boolean;
  canEdit: boolean;
  canArchive: boolean;
  dmPending: boolean;
  dmHref: string;
  onDm: (e: React.MouseEvent<HTMLAnchorElement>) => void;
  onAssign: () => void;
  onArchive?: () => void;
  onUpdate: (id: string, data: Record<string, unknown>) => Promise<void>;
}

/**
 * Compact 320px inspector card — the original right-hand summary chrome,
 * plus avatar/name/presence and DM / assign / edit. Not a page-wide form.
 */
export function AgentIdentityCard({
  agent,
  runtime,
  owner,
  presence,
  canAssign,
  canEdit,
  canArchive,
  dmPending,
  dmHref,
  onDm,
  onAssign,
  onArchive,
  onUpdate,
}: AgentIdentityCardProps) {
  const { t } = useT("agents");
  const isArchived = !!agent.archived_at;
  const runtimeOnline = runtime?.status === "online";
  const [editing, setEditing] = useState(false);
  const [name, setName] = useState(agent.name);
  const [saving, setSaving] = useState(false);
  const modelsQuery = useQuery(
    runtimeModelsOptions(
      runtimeOnline ? runtime?.id : null,
      runtime?.workspace_id,
    ),
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
    setName(agent.name);
    setEditing(false);
  }, [agent.id]);

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
      setName(agent.name);
    } finally {
      setSaving(false);
    }
  };

  return (
    <aside className="w-[320px] self-start rounded-xl border border-surface-border bg-surface p-5 shadow-[var(--surface-shadow)] xl:sticky xl:top-6">
      <div className="flex items-start justify-between gap-2">
        <h2 className="text-body font-medium">
          {t(($) => $.overview.agent_context)}
        </h2>
        {canEdit ? (
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
        ) : null}
      </div>

      <div className="mt-4 flex items-start gap-3">
        {editing && canEdit ? (
          <AvatarUploadControl
            variant="agent"
            value={agent.avatar_url ?? null}
            name={agent.name}
            size={40}
            editBadge
            disabled={saving}
            ariaLabel={t(($) => $.inspector.change_avatar_aria)}
            onUploaded={(url) => onUpdate(agent.id, { avatar_url: url })}
            onEmojiSelected={(value) =>
              void onUpdate(agent.id, { avatar_url: value })
            }
          />
        ) : (
          <ActorAvatar
            actorType="agent"
            actorId={agent.id}
            size="lg"
            profileLink={false}
            className="ring-1 ring-border"
          />
        )}
        <div className="min-w-0 flex-1">
          {editing ? (
            <div>
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
                className="h-8 text-caption"
              />
              {nameInvalid ? (
                <p className="mt-1 text-caption text-destructive">
                  {t(($) => $.inspector.rename_required)}
                </p>
              ) : null}
            </div>
          ) : (
            <p
              className="truncate text-body font-medium"
              data-testid="agent-name-value"
              title={agent.name}
            >
              {agent.name}
            </p>
          )}
          <div className="mt-1">
            <AgentPresenceIndicator detail={presence} />
          </div>
        </div>
      </div>

      {!isArchived && (
        <div className="mt-4 flex flex-wrap gap-2">
          <Button
            variant="outline"
            size="sm"
            disabled={dmPending}
            className="h-7 px-2 text-caption data-disabled:pointer-events-none data-disabled:opacity-50"
            render={<AppLink href={dmHref} onClick={onDm} />}
            nativeButton={false}
          >
            <MessageSquare className="h-3.5 w-3.5" aria-hidden="true" />
            {t(($) => $.detail.dm)}
          </Button>
          {canAssign ? (
            <Button
              type="button"
              size="sm"
              className="h-7 px-2 text-caption"
              onClick={onAssign}
            >
              <Plus className="h-3.5 w-3.5" aria-hidden="true" />
              {t(($) => $.detail.assign_work)}
            </Button>
          ) : null}
          {canArchive && onArchive ? (
            <DropdownMenu>
              <DropdownMenuTrigger
                render={<Button variant="ghost" size="icon-sm" className="h-7 w-7" />}
                aria-label={t(($) => $.detail.more_actions_aria)}
              >
                <MoreHorizontal
                  className="h-3.5 w-3.5 text-muted-foreground"
                  aria-hidden="true"
                />
              </DropdownMenuTrigger>
              <DropdownMenuContent align="end" className="w-auto">
                <DropdownMenuItem variant="destructive" onClick={onArchive}>
                  <Trash2 className="h-3.5 w-3.5" aria-hidden="true" />
                  {t(($) => $.detail.more_archive)}
                </DropdownMenuItem>
              </DropdownMenuContent>
            </DropdownMenu>
          ) : null}
        </div>
      )}

      <dl className="mt-5 space-y-3 text-caption">
        {owner ? (
          <SummaryRow label={t(($) => $.inspector.prop_owner)}>
            <span className="flex min-w-0 items-center gap-1.5">
              <ActorAvatar
                actorType="member"
                actorId={owner.user_id}
                size="xs"
              />
              <span className="truncate text-foreground">{owner.name}</span>
            </span>
          </SummaryRow>
        ) : null}
        <SummaryRow label={t(($) => $.overview.access)}>
          <VisibilityBadge value={agent.visibility} />
        </SummaryRow>
        <SummaryRow label={t(($) => $.inspector.prop_runtime)}>
          <span className="flex min-w-0 items-center gap-1.5 text-foreground">
            <span
              className={`h-1.5 w-1.5 shrink-0 rounded-full ${
                runtimeOnline ? "bg-success" : "bg-muted-foreground/40"
              }`}
              aria-hidden="true"
            />
            <Server
              className="h-3 w-3 shrink-0 text-muted-foreground"
              aria-hidden="true"
            />
            <span className="truncate">
              {runtime
                ? runtimeDisplayLabel(runtime)
                : t(($) => $.pickers.runtime_none)}
            </span>
          </span>
        </SummaryRow>
        <SummaryRow label={t(($) => $.inspector.prop_model)}>
          <span className="flex min-w-0 items-center gap-1.5 text-foreground">
            <Bot
              className="h-3 w-3 shrink-0 text-muted-foreground"
              aria-hidden="true"
            />
            <span className="truncate">{modelSummary}</span>
          </span>
        </SummaryRow>
        <SummaryRow label={t(($) => $.inspector.prop_concurrency)}>
          <span className="font-mono tabular-nums text-foreground">
            {agent.max_concurrent_tasks}
          </span>
        </SummaryRow>
      </dl>
    </aside>
  );
}

function SummaryRow({
  label,
  children,
}: {
  label: string;
  children: React.ReactNode;
}) {
  return (
    <div className="grid grid-cols-[88px_minmax(0,1fr)] items-center gap-3">
      <dt className="text-muted-foreground">{label}</dt>
      <dd className="min-w-0">{children}</dd>
    </div>
  );
}
