"use client";

import { useEffect, useState } from "react";
import { AlertCircle, ArrowLeft, Lock, Server } from "lucide-react";
import { toast } from "sonner";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import type { Agent, UpdateAgentRequest } from "@orvilo/core/types";
import {
  type AgentPresenceDetail,
  isAgentRuntimeBound,
  useWorkspacePresenceMap,
} from "@orvilo/core/agents";
import { api, ApiError } from "@orvilo/core/api";
import { useAuthStore } from "@orvilo/core/auth";
import { useChatStore } from "@orvilo/core/chat";
import { useWorkspaceId } from "@orvilo/core/hooks";
import { useWorkspacePaths } from "@orvilo/core/paths";
import {
  agentDetailOptions,
  agentListOptions,
  cacheAgentResponse,
  memberListOptions,
  workspaceKeys,
} from "@orvilo/core/workspace/queries";
import { runtimeListOptions } from "@orvilo/core/runtimes";
import { useAgentPermissions } from "@orvilo/core/permissions";
import { Button } from "@orvilo/ui/components/ui/button";
import { CapabilityBanner } from "@orvilo/ui/components/common/capability-banner";
import { Skeleton } from "@orvilo/ui/components/ui/skeleton";
import { AppLink } from "../../navigation";
import { PageHeader } from "../../layout/page-header";
import { AgentOverviewPane, type DetailTab } from "./agent-overview-pane";
import { AgentIdentityCard } from "./agent-identity-card";
import { useT } from "../../i18n";

interface AgentDetailPageProps {
  agentId: string;
}

export function AgentDetailPage({ agentId }: AgentDetailPageProps) {
  const { t } = useT("agents");
  const wsId = useWorkspaceId();
  const paths = useWorkspacePaths();
  const qc = useQueryClient();
  const currentUser = useAuthStore((s) => s.user);

  const {
    data: agents = [],
    isLoading: agentsLoading,
    error: agentsError,
    refetch: refetchAgents,
  } = useQuery(agentListOptions(wsId));
  const { data: runtimes = [] } = useQuery(runtimeListOptions(wsId));
  const { data: members = [] } = useQuery(memberListOptions(wsId));

  // Single workspace-level presence pass; this page just reads its slot.
  // The hook owns the 30s tick so the failed-window auto-clears here too.
  const { byAgent: presenceMap } = useWorkspacePresenceMap(wsId);

  const listAgent = agents.find((a) => a.id === agentId) ?? null;

  // The list remains the zero-request common path. When it has settled
  // without the requested agent, use the canonical detail query to resolve
  // direct links and distinguish 403/404 from transient failures. Creation
  // hydrates this same key, so a newly-created agent renders immediately.
  const detailQuery = useQuery({
    ...agentDetailOptions(wsId, agentId),
    enabled: !agentsLoading && !listAgent && !!agentId,
  });
  const detailError = detailQuery.error;
  const isForbidden =
    detailError instanceof ApiError && detailError.status === 403;
  const isNotFound =
    detailError instanceof ApiError && detailError.status === 404;
  // TanStack intentionally keeps successful data when a refetch fails. Do not
  // let that stale snapshot mask a later 403/404 after access is revoked or the
  // agent is deleted, but preserve it through transient network failures. A
  // still-visible list response remains authoritative in either case.
  const agent =
    listAgent ?? (isForbidden || isNotFound ? null : detailQuery.data) ?? null;
  const presence: AgentPresenceDetail | null = agent
    ? (presenceMap.get(agent.id) ?? null)
    : null;

  // Permission hook MUST be called unconditionally — its `agent | null`
  // signature handles the not-found / loading case internally so the early
  // returns below don't violate the rules of hooks. Backend gates archive
  // and restore identically to edit, so a single `canEdit` covers them all.
  const {
    canAssign,
    canEdit,
    isLoading: permissionsLoading,
  } = useAgentPermissions(agent, wsId);
  const setAgentDetailDmAvailable = useChatStore(
    (state) => state.setAgentDetailDmAvailable,
  );
  const isArchived = !!agent?.archived_at;
  const runtimeBound = agent ? isAgentRuntimeBound(agent) : false;
  const agentDetailDmAvailable =
    !!agent &&
    !isArchived &&
    !permissionsLoading &&
    canAssign.allowed &&
    runtimeBound;

  useEffect(() => {
    setAgentDetailDmAvailable(agentDetailDmAvailable);
    return () => setAgentDetailDmAvailable(false);
  }, [agentDetailDmAvailable, setAgentDetailDmAvailable]);

  // One-shot channel: the inspector's compact Lark status row asks the
  // overview pane to focus a tab. The pane clears it after consuming.
  const [tabNavIntent, setTabNavIntent] = useState<DetailTab | null>(null);

  const handleUpdate = async (id: string, data: Record<string, unknown>) => {
    // Optimistic update: patch the matching agent in the cached list
    // BEFORE the network round-trip so the inspector picker chips flip to
    // the new value immediately on click. Without this, every inspector
    // picker (thinking / visibility / concurrency / model / runtime) waits
    // 0.5-2s for the API response + invalidate + refetch before the trigger
    // updates — readable as obvious lag in the UI.
    //
    // On error we rollback only the fields THIS call wrote, leaving any
    // other concurrently-mutated fields untouched, then invalidate so the
    // cache converges with the server. A whole-list snapshot rollback
    // would clobber a concurrent successful mutation if the failing call
    // resolves last (e.g. flipping visibility then runtime simultaneously
    // and only the visibility PATCH fails).
    const optimisticData =
      typeof data.runtime_id === "string"
        ? { ...data, runtime_bound: data.runtime_id.trim().length > 0 }
        : data;
    const queryKey = workspaceKeys.agents(wsId);
    const detailQueryKey = workspaceKeys.agent(wsId, id);
    const prevAgents = qc.getQueryData<Agent[]>(queryKey);
    const prevListAgent = prevAgents?.find((a) => a.id === id);
    const prevDetailAgent = qc.getQueryData<Agent>(detailQueryKey);
    const previousFields = (previousAgent: Agent | undefined) => {
      const fields: Record<string, unknown> = {};
      if (!previousAgent) return fields;
      for (const key of Object.keys(optimisticData)) {
        fields[key] = (previousAgent as unknown as Record<string, unknown>)[
          key
        ];
      }
      return fields;
    };
    const prevListFields = previousFields(prevListAgent);
    const prevDetailFields = previousFields(prevDetailAgent);
    qc.setQueryData<Agent[]>(queryKey, (old) =>
      old?.map((a) =>
        a.id === id ? ({ ...a, ...optimisticData } as Agent) : a,
      ),
    );
    qc.setQueryData<Agent>(detailQueryKey, (old) =>
      old ? ({ ...old, ...optimisticData } as Agent) : old,
    );
    try {
      const updatedAgent = await api.updateAgent(
        id,
        data as UpdateAgentRequest,
      );
      cacheAgentResponse(qc, wsId, updatedAgent, { insertIntoList: false });
      void qc.invalidateQueries({ queryKey });
      toast.success(t(($) => $.detail.agent_updated_toast));
    } catch (e) {
      if (prevListAgent) {
        qc.setQueryData<Agent[]>(queryKey, (old) =>
          old?.map((a) =>
            a.id === id ? ({ ...a, ...prevListFields } as Agent) : a,
          ),
        );
      }
      if (prevDetailAgent) {
        qc.setQueryData<Agent>(detailQueryKey, (old) =>
          old ? ({ ...old, ...prevDetailFields } as Agent) : old,
        );
      }
      void qc.invalidateQueries({ queryKey });
      toast.error(
        e instanceof Error ? e.message : t(($) => $.detail.update_failed_toast),
      );
      throw e;
    }
  };

  const handleRestore = async (id: string) => {
    try {
      await api.restoreAgent(id);
      qc.invalidateQueries({ queryKey: workspaceKeys.agents(wsId) });
      toast.success(t(($) => $.detail.agent_restored_toast));
    } catch (e) {
      toast.error(
        e instanceof Error
          ? e.message
          : t(($) => $.detail.restore_failed_toast),
      );
    }
  };

  // --- Loading ---
  if (!agent && (agentsLoading || detailQuery.isPending)) {
    return <DetailLoadingSkeleton />;
  }

  // --- No permission (private agent the caller is not in allowed_principals for) ---
  if (!agent && isForbidden) {
    return (
      <div className="flex flex-1 min-h-0 flex-col">
        <BackHeader
          paths={paths.agents()}
          title={t(($) => $.detail.back_to_agents)}
        />
        <div className="flex flex-1 flex-col items-center justify-center gap-3 px-6 py-16 text-center">
          <Lock className="h-8 w-8 text-muted-foreground" />
          <div>
            <p className="text-body font-medium">
              {t(($) => $.detail.no_access_title)}
            </p>
            <p className="mt-1 text-caption text-muted-foreground">
              {t(($) => $.detail.no_access_hint)}
            </p>
          </div>
          <Button
            size="sm"
            render={<AppLink href={paths.agents()} />}
            nativeButton={false}
          >
            {t(($) => $.detail.back_to_agents_full)}
          </Button>
        </div>
      </div>
    );
  }

  // --- Not found / error ---
  if (!agent) {
    const loadError = detailError ?? agentsError;
    return (
      <div className="flex flex-1 min-h-0 flex-col">
        <BackHeader
          paths={paths.agents()}
          title={t(($) => $.detail.back_to_agents)}
        />
        <div className="flex flex-1 flex-col items-center justify-center gap-3 px-6 py-16 text-center">
          <AlertCircle className="h-8 w-8 text-destructive" />
          <div>
            <p className="text-body font-medium">
              {isNotFound
                ? t(($) => $.detail.not_found_title)
                : t(($) => $.detail.load_failed_title)}
            </p>
            <p className="mt-1 text-caption text-muted-foreground">
              {isNotFound
                ? t(($) => $.detail.not_found_default)
                : loadError instanceof Error
                  ? loadError.message
                  : t(($) => $.detail.load_failed_default)}
            </p>
          </div>
          <div className="flex items-center gap-2">
            <Button
              type="button"
              variant="outline"
              size="sm"
              onClick={() => {
                void Promise.all([refetchAgents(), detailQuery.refetch()]);
              }}
            >
              {t(($) => $.detail.try_again)}
            </Button>
            <Button
              size="sm"
              render={<AppLink href={paths.agents()} />}
              nativeButton={false}
            >
              {t(($) => $.detail.back_to_agents_full)}
            </Button>
          </div>
        </div>
      </div>
    );
  }

  const runtime = runtimeBound
    ? (runtimes.find((r) => r.id === agent.runtime_id) ?? null)
    : null;
  const owner = agent.owner_id
    ? (members.find((m) => m.user_id === agent.owner_id) ?? null)
    : null;

  // Chat shares the invocation gate with assignment (MUL-3963): starting a
  // chat triggers agent runs. The button stays visible either way — a denied
  // click explains itself instead of the affordance silently missing. While
  // membership is still resolving the decision is undetermined, so the button
  // is disabled rather than toasting a false "no access" at a real member.
  //
  // The control is a real link, so a failed gate has to cancel the navigation
  // AppLink would otherwise perform — preventDefault is that cancel.
  const handleDm = (e: React.MouseEvent<HTMLAnchorElement>) => {
    if (permissionsLoading) {
      e.preventDefault();
      return;
    }
    if (!canAssign.allowed) {
      e.preventDefault();
      toast.error(t(($) => $.detail.dm_no_permission_toast));
      return;
    }
    if (!runtimeBound) {
      e.preventDefault();
      toast.error(t(($) => $.detail.runtime_required_toast));
    }
  };
  return (
    <div className="flex min-h-0 flex-1 flex-col bg-background text-foreground">
      <AgentIdentityCard
        breadcrumbHref={paths.agents()}
        agent={agent}
        runtime={runtime}
        owner={owner}
        presence={presence}
        canEdit={canEdit.allowed}
        dmPending={permissionsLoading}
        dmHref={`${paths.chat()}?agent=${agent.id}`}
        onDm={handleDm}
        onUpdate={handleUpdate}
      />

      {!canEdit.allowed && (
        <div className="px-6 pt-3">
          <CapabilityBanner
            reason={canEdit.reason}
            resource="agent"
            ownerName={owner?.name}
          />
        </div>
      )}

      {isArchived && (
        <div className="flex shrink-0 items-center gap-2 border-b bg-muted/50 px-6 py-2 text-caption text-muted-foreground">
          <AlertCircle className="h-3.5 w-3.5 shrink-0" />
          <span className="flex-1">{t(($) => $.detail.archived_banner)}</span>
          {canEdit.allowed && (
            <Button
              variant="outline"
              size="sm"
              className="h-6 text-caption"
              onClick={() => handleRestore(agent.id)}
            >
              {t(($) => $.detail.restore)}
            </Button>
          )}
        </div>
      )}

      {!isArchived && !runtimeBound && (
        <div className="flex shrink-0 items-center gap-2 border-b border-amber-500/30 bg-amber-500/10 px-6 py-2 text-caption text-amber-900 dark:text-amber-100">
          <Server className="h-3.5 w-3.5 shrink-0" />
          <span className="flex-1">
            {t(($) => $.detail.runtime_required_banner)}
          </span>
          {canEdit.allowed && (
            <Button
              variant="outline"
              size="sm"
              className="h-6 border-amber-500/40 bg-background/70 text-caption"
              onClick={() => setTabNavIntent("general")}
            >
              {t(($) => $.detail.bind_runtime)}
            </Button>
          )}
        </div>
      )}

      <div className="flex min-h-0 min-w-0 flex-1 flex-col overflow-hidden">
          <AgentOverviewPane
            agent={agent}
            runtime={runtime}
            owner={owner}
            runtimes={runtimes}
            members={members}
            onUpdate={handleUpdate}
            currentUserId={currentUser?.id ?? null}
            canEdit={canEdit.allowed}
            navIntent={tabNavIntent}
            onNavIntentHandled={() => setTabNavIntent(null)}
          />
      </div>
    </div>
  );
}

function BackHeader({ paths, title }: { paths: string; title: string }) {
  return (
    <PageHeader>
      <AppLink
        href={paths}
        className="inline-flex h-7 items-center gap-1 rounded-md px-2 text-caption text-muted-foreground transition-colors hover:bg-accent hover:text-foreground"
      >
        <ArrowLeft className="h-3.5 w-3.5" />
        {title}
      </AppLink>
    </PageHeader>
  );
}

function DetailLoadingSkeleton() {
  return (
    <div className="flex flex-1 min-h-0 flex-col">
      <div className="shrink-0 px-6 pt-3">
        <Skeleton className="h-4 w-48" />
      </div>
      <div className="mt-4 flex min-h-0 flex-1 flex-col">
        <div className="min-w-0 flex-1 space-y-4 p-6">
          <Skeleton className="h-8 w-40" />
          <Skeleton className="h-64 w-full" />
        </div>
      </div>
    </div>
  );
}
