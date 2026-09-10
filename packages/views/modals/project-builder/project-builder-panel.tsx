"use client";

import { useEffect, useMemo, useRef, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { ChevronDown, Loader2, MessageSquare } from "lucide-react";
import {
  Plan,
  PlanAction,
  PlanContent,
  PlanDescription,
  PlanHeader,
  PlanTitle,
} from "@orvilo/ui/components/ai-elements/plan";
import {
  Confirmation,
  ConfirmationAction,
  ConfirmationActions,
  ConfirmationRequest,
  ConfirmationTitle,
} from "@orvilo/ui/components/ai-elements/confirmation";
import { useAuthStore } from "@orvilo/core/auth";
import { useWorkspaceId } from "@orvilo/core/hooks";
import { useCurrentWorkspace } from "@orvilo/core/paths";
import {
  isAgentRuntimeBound,
  useAgentPresenceDetail,
} from "@orvilo/core/agents";
import { canAssignAgent } from "@orvilo/views/issues/components";
import {
  agentListOptions,
  memberListOptions,
} from "@orvilo/core/workspace/queries";
import { labelListOptions } from "@orvilo/core/labels/queries";
import { projectListOptions } from "@orvilo/core/projects/queries";
import type { Agent, MemberWithUser, Project } from "@orvilo/core/types";
import { ActorAvatar } from "../../common/actor-avatar";
import { AgentPicker } from "../../chat/components/new-chat-button";
import {
  ChatMessageList,
  ChatMessageSkeleton,
} from "../../chat/components/chat-message-list";
import { ChatInput } from "../../chat/components/chat-input";
import { useT } from "../../i18n";
import {
  decodeProjectBuilderInput,
  mergeProjectBuilderDraft,
  parseProjectBuilderDraft,
  stripProjectBuilderDraft,
  type ProjectBuilderCatalogItem,
  type ProjectBuilderCatalogs,
  type ProjectBuilderDraft,
} from "./project-builder-draft";
import { useProjectBuilderSession } from "./use-project-builder-session";

export interface ProjectBuilderPanelProps {
  /** The current form state, used as the assistant's first-turn context. */
  currentDraft: ProjectBuilderDraft;
  /** Called once when the user explicitly applies the assistant proposal. */
  onApply: (draft: ProjectBuilderDraft) => void;
}

type WorkspaceWithLeadAgent = { lead_agent_id?: string | null };

function catalogItem(id: string, name: string): ProjectBuilderCatalogItem {
  return { id, name };
}

function currentMemberCatalog(members: MemberWithUser[]): ProjectBuilderCatalogItem[] {
  return members.map((member) => catalogItem(member.user_id, member.name));
}

function currentAgentCatalog(agents: Agent[]): ProjectBuilderCatalogItem[] {
  return agents
    .filter((agent) => !agent.archived_at)
    .map((agent) => catalogItem(agent.id, agent.name));
}

function currentProjectCatalog(projects: Project[]): ProjectBuilderCatalogItem[] {
  return projects.map((project) => catalogItem(project.id, project.title));
}

function workspaceLeadAgentId(
  workspace: ReturnType<typeof useCurrentWorkspace>,
): string | null {
  if (!workspace) return null;
  const leadAgentId = (workspace as WorkspaceWithLeadAgent).lead_agent_id;
  return typeof leadAgentId === "string" && leadAgentId.trim()
    ? leadAgentId
    : null;
}

export function ProjectBuilderPanel({
  currentDraft,
  onApply,
}: ProjectBuilderPanelProps) {
  const { t } = useT("modals");
  const wsId = useWorkspaceId();
  const workspace = useCurrentWorkspace();
  const currentUserId = useAuthStore((state) => state.user?.id ?? null);
  const { data: members = [] } = useQuery(memberListOptions(wsId));
  const { data: agents = [] } = useQuery(agentListOptions(wsId));
  const { data: labels = [] } = useQuery(labelListOptions(wsId, "project"));
  const { data: projects = [] } = useQuery(projectListOptions(wsId));
  const memberRole = members.find((member) => member.user_id === currentUserId)?.role;
  const invokableAgents = useMemo(
    () =>
      agents.filter(
        (agent) =>
          !agent.archived_at && canAssignAgent(agent, currentUserId ?? undefined, memberRole),
      ),
    [agents, currentUserId, memberRole],
  );
  const leadAgentId = workspaceLeadAgentId(workspace);
  const [selectedAgentId, setSelectedAgentId] = useState<string | null>(null);

  // A workspace lead is only a default. It must pass the same invocation gate as
  // the picker; never silently choose the first agent in the workspace.
  useEffect(() => {
    if (selectedAgentId !== null || !leadAgentId) return;
    if (invokableAgents.some((agent) => agent.id === leadAgentId)) {
      setSelectedAgentId(leadAgentId);
    }
  }, [invokableAgents, leadAgentId, selectedAgentId]);

  // Keep the full row for read-only/error states if permissions or archive state
  // changes while the panel is open. The picker itself only receives invokable
  // agents, so a revoked agent cannot be selected for a new session.
  const selectedAgent = useMemo(
    () =>
      selectedAgentId
        ? agents.find((agent) => agent.id === selectedAgentId) ?? null
        : null,
    [agents, selectedAgentId],
  );
  const agentAccessRevoked =
    !!selectedAgent &&
    !selectedAgent.archived_at &&
    !canAssignAgent(selectedAgent, currentUserId ?? undefined, memberRole);
  const agentArchived = !!selectedAgent?.archived_at;
  const agentRuntimeBound = !!selectedAgent && isAgentRuntimeBound(selectedAgent);
  const presence = useAgentPresenceDetail(wsId, selectedAgent?.id);
  const availability = presence === "loading" ? undefined : presence.availability;

  const catalogs = useMemo<ProjectBuilderCatalogs>(
    () => ({
      members: currentMemberCatalog(members),
      agents: currentAgentCatalog(invokableAgents),
      labels: labels.map((label) => catalogItem(label.id, label.name)),
      projects: currentProjectCatalog(projects),
    }),
    [invokableAgents, labels, members, projects],
  );

  const [proposedDraft, setProposedDraft] = useState(currentDraft);
  const [hasSuggestion, setHasSuggestion] = useState(false);
  const [applying, setApplying] = useState(false);
  const [panelError, setPanelError] = useState<string | null>(null);
  const projectBuilder = useProjectBuilderSession({
    agent: selectedAgent,
    draft: proposedDraft,
    catalogs,
  });
  const sessionId = projectBuilder.sessionId;

  // Keep the initial preview aligned with edits in the main form until a real
  // assistant session starts. Once the conversation exists, its proposal is
  // the panel's own review state.
  useEffect(() => {
    if (!sessionId) setProposedDraft(currentDraft);
  }, [currentDraft, sessionId]);

  const appliedMessageIdRef = useRef<string | null>(null);
  const latestDraftMessage = [...projectBuilder.messages]
    .reverse()
    .find(
      (message) =>
        message.role === "assistant" && parseProjectBuilderDraft(message.content),
    );

  useEffect(() => {
    if (!latestDraftMessage || latestDraftMessage.id === appliedMessageIdRef.current) {
      return;
    }
    const payload = parseProjectBuilderDraft(latestDraftMessage.content);
    if (!payload) return;
    appliedMessageIdRef.current = latestDraftMessage.id;
    setProposedDraft((current) => mergeProjectBuilderDraft(current, payload, catalogs));
    setHasSuggestion(true);
  }, [catalogs, latestDraftMessage]);

  const handleApply = () => {
    if (!hasSuggestion || applying) return;
    setApplying(true);
    setPanelError(null);
    try {
      onApply(proposedDraft);
      setHasSuggestion(false);
    } finally {
      setApplying(false);
    }
  };

  const displayMessages = useMemo(
    () =>
      projectBuilder.messages.map((message) => ({
        ...message,
        content:
          message.role === "user"
            ? decodeProjectBuilderInput(message.content)
            : stripProjectBuilderDraft(message.content),
      })),
    [projectBuilder.messages],
  );
  const error = panelError ?? projectBuilder.error;
  const composerDisabled =
    projectBuilder.starting ||
    !selectedAgent ||
    agentArchived ||
    agentAccessRevoked ||
    !agentRuntimeBound;
  const composerKey = sessionId;

  const agentPicker = (
    <AgentPicker
      agents={invokableAgents}
      userId={currentUserId ?? undefined}
      currentAgentId={selectedAgent?.id}
      onSelect={(agent) => setSelectedAgentId(agent.id)}
      triggerRender={
        <button
          type="button"
          disabled={projectBuilder.starting}
          aria-label={t(($) => $.create_issue.agent.select_agent_aria)}
          className="flex min-w-0 max-w-52 items-center gap-1.5 rounded-md px-1.5 py-1 text-caption outline-none transition-colors hover:bg-accent aria-expanded:bg-accent disabled:pointer-events-none disabled:opacity-60"
        />
      }
      trigger={
        selectedAgent ? (
          <>
            <ActorAvatar
              actorType="agent"
              actorId={selectedAgent.id}
              size="sm"
              showStatusDot
            />
            <span className="truncate">{selectedAgent.name}</span>
          </>
        ) : (
          <span className="truncate text-muted-foreground">
            {t(($) => $.create_issue.agent.select_agent_aria)}
          </span>
        )
      }
    />
  );

  return (
    <aside
      aria-label={t(($) => $.create_project.create_with_agent)}
      className="flex h-full min-h-0 min-w-0 w-full flex-col bg-background"
    >
      <header className="flex min-h-14 shrink-0 items-center justify-between gap-3 border-b px-4 py-3">
        <div className="flex min-w-0 items-center gap-3">
          <MessageSquare className="size-4 shrink-0 text-primary" aria-hidden="true" />
          <div className="min-w-0">
            <h2 className="truncate text-body font-semibold">
              {t(($) => $.create_project.create_with_agent)}
            </h2>
            <p className="truncate text-caption text-muted-foreground">
              {hasSuggestion
                ? t(($) => $.create_project.builder_suggestion)
                : sessionId
                  ? t(($) => $.create_project.builder_ready)
                  : t(($) => $.create_project.builder_intro)}
            </p>
          </div>
        </div>
        <div className="flex min-w-0 shrink-0 items-center gap-1">
          {sessionId ? (
            <div className="flex min-w-0 max-w-52 items-center gap-1.5 px-1.5 py-1 text-caption text-muted-foreground">
              {selectedAgent ? (
                <>
                  <ActorAvatar
                    actorType="agent"
                    actorId={selectedAgent.id}
                    size="sm"
                    showStatusDot
                  />
                  <span className="truncate">{selectedAgent.name}</span>
                </>
              ) : (
                <span>{t(($) => $.create_issue.agent.select_agent_aria)}</span>
              )}
            </div>
          ) : (
            <div className="flex items-center gap-1">
              {agentPicker}
              <ChevronDown className="pointer-events-none -ml-5 size-3 text-muted-foreground" aria-hidden="true" />
            </div>
          )}
        </div>
      </header>

      <div className="flex min-h-0 flex-1 flex-col">
        {projectBuilder.messagesLoading ? (
          <ChatMessageSkeleton />
        ) : sessionId && (displayMessages.length > 0 || projectBuilder.pending) ? (
          <ChatMessageList
            key={sessionId}
            messages={displayMessages}
            pendingTask={projectBuilder.pendingTask}
            availability={availability}
            transformContent={stripProjectBuilderDraft}
          />
        ) : (
          <div className="flex min-h-0 flex-1 items-center justify-center overflow-y-auto px-5 py-8">
            <div className="w-full max-w-sm text-center">
              <MessageSquare className="mx-auto size-7 text-faint-foreground" aria-hidden="true" />
              <p className="mt-3 text-body leading-6 text-muted-foreground">
                {t(($) => $.create_project.builder_intro)}
              </p>
              {!selectedAgent ? (
                <p className="mt-2 text-caption text-muted-foreground">
                  {t(($) => $.create_issue.agent.select_agent_aria)}
                </p>
              ) : null}
            </div>
          </div>
        )}
      </div>

      {hasSuggestion ? (
        <div className="border-t bg-muted/20 px-4 py-3">
          <Confirmation
            approval={{ id: latestDraftMessage?.id ?? "project-draft" }}
            state="approval-requested"
            className="border-0 bg-transparent p-0"
          >
            <ConfirmationRequest>
              <ConfirmationTitle className="sr-only">
                {t(($) => $.create_project.builder_suggestion)}
              </ConfirmationTitle>
            </ConfirmationRequest>
            <Plan defaultOpen className="border-0 bg-transparent py-0">
            <PlanHeader className="px-0 py-0">
              <div className="min-w-0">
                <PlanTitle>{proposedDraft.title || t(($) => $.create_project.builder_suggestion)}</PlanTitle>
                <PlanDescription>
                  {proposedDraft.summary || t(($) => $.create_project.builder_suggestion)}
                </PlanDescription>
              </div>
              <PlanAction>
                <ConfirmationActions>
                  <ConfirmationAction
                    onClick={handleApply}
                    disabled={applying || projectBuilder.pending}
                  >
                    {applying ? <Loader2 className="size-4 animate-spin" /> : null}
                    {t(($) => $.create_project.builder_apply)}
                  </ConfirmationAction>
                </ConfirmationActions>
              </PlanAction>
            </PlanHeader>
            <PlanContent className="px-0 pb-0 pt-2 text-caption text-muted-foreground">
              <span>{proposedDraft.status}</span>
              <span aria-hidden="true"> · </span>
              <span>{proposedDraft.priority}</span>
              {proposedDraft.due_date ? <span> · {proposedDraft.due_date}</span> : null}
            </PlanContent>
            </Plan>
          </Confirmation>
        </div>
      ) : null}
      {error ? (
        <p role="alert" className="border-t px-4 py-2 text-caption text-destructive">
          {error}
        </p>
      ) : null}
      <ChatInput
        onSend={(content, attachmentIds, commitInput, attachments) =>
          projectBuilder.send(
            content,
            attachmentIds,
            attachments,
            () => commitInput(),
          )
        }
        onStop={() => void projectBuilder.stop()}
        isRunning={projectBuilder.pending}
        disabled={composerDisabled}
        noAgent={!selectedAgent}
        agentArchived={agentArchived}
        agentAccessRevoked={agentAccessRevoked}
        agentRuntimeRequired={!!selectedAgent && !agentRuntimeBound}
        uploadEnabled={!!selectedAgent && !agentArchived && !agentAccessRevoked}
        agentName={selectedAgent?.name}
        draftKeyOverride={composerKey}
        editorKeyOverride={composerKey}
        restoreDraftRequest={projectBuilder.restoreDraftRequest}
        onRestoreDraftApplied={projectBuilder.handleRestoreDraftApplied}
      />
    </aside>
  );
}
