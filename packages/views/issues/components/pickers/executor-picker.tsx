"use client";

import { useMemo, useState } from "react";
import { Lock, UserMinus } from "lucide-react";
import { useQuery } from "@tanstack/react-query";
import type {
  Agent,
  IssueActorType,
  IssueExecutorType,
  IssueOwnerType,
  IssueReviewerType,
  UpdateIssueRequest,
} from "@patchbay/core/types";
import { useAuthStore } from "@patchbay/core/auth";
import { isAgentRuntimeBound } from "@patchbay/core/agents";
import { canAssignAgentToIssue } from "@patchbay/core/permissions";
import { useWorkspaceId } from "@patchbay/core/hooks";
import { useActorName } from "@patchbay/core/workspace/hooks";
import {
  agentListOptions,
  executorFrequencyOptions,
  memberListOptions,
  teamListOptions,
} from "@patchbay/core/workspace/queries";
import { ActorAvatar } from "../../../common/actor-avatar";
import { DeferredPopup } from "../../../common/deferred-popup";
import { matchesPinyin } from "../../../editor/extensions/pinyin-match";
import { useT } from "../../../i18n";
import {
  PICKER_TRIGGER_CLASS,
  PickerEmpty,
  PickerItem,
  PickerSection,
  PropertyPicker,
} from "./property-picker";

export function canAssignAgent(
  agent: Agent,
  userId: string | undefined,
  memberRole: string | undefined,
): boolean {
  return canAssignAgentToIssue(agent, {
    userId: userId ?? null,
    role:
      memberRole === "owner" || memberRole === "admin" || memberRole === "member"
        ? memberRole
        : null,
  }).allowed;
}

type SharedPickerProps = {
  mixed?: boolean;
  trigger?: React.ReactNode;
  triggerRender?: React.ReactElement<Record<string, unknown>>;
  open?: boolean;
  onOpenChange?: (open: boolean) => void;
  align?: "start" | "center" | "end";
  allowUnassigned?: boolean;
};

type ActorRolePickerProps = SharedPickerProps & {
  actorType: IssueActorType | null;
  actorId: string | null;
  allowedKinds: readonly IssueActorType[];
  onSelect: (type: IssueActorType | null, id: string | null) => void;
  emptyLabel: string;
  searchPlaceholder: string;
};

type ExecutorPickerProps = SharedPickerProps & {
  executorType: IssueExecutorType | null;
  executorId: string | null;
  onUpdate: (updates: Partial<UpdateIssueRequest>) => void;
};

type OwnerPickerProps = SharedPickerProps & {
  ownerType: IssueOwnerType | null;
  ownerId: string | null;
  onUpdate: (updates: Partial<UpdateIssueRequest>) => void;
};

type ReviewerPickerProps = SharedPickerProps & {
  reviewerType: IssueReviewerType | null;
  reviewerId: string | null;
  onUpdate: (updates: Partial<UpdateIssueRequest>) => void;
};

/** Human responsibility is intentionally separate from agent execution. */
export function OwnerPicker({ ownerType, ownerId, onUpdate, ...props }: OwnerPickerProps) {
  const { t } = useT("issues");
  return (
    <ActorRolePicker
      {...props}
      actorType={ownerType}
      actorId={ownerId}
      allowedKinds={["member"]}
      emptyLabel={t(($) => $.pickers.owner.trigger_unassigned)}
      searchPlaceholder={t(($) => $.pickers.owner.search_placeholder)}
      onSelect={(type, id) =>
        onUpdate({
          owner_type: type === "member" ? type : null,
          owner_id: type === "member" ? id : null,
        })
      }
    />
  );
}

/** Execution accepts runnable agents and teams only; people belong in owner. */
export function ExecutorPicker({ executorType, executorId, onUpdate, ...props }: ExecutorPickerProps) {
  const { t } = useT("issues");
  return (
    <ActorRolePicker
      {...props}
      actorType={executorType}
      actorId={executorId}
      allowedKinds={["agent", "team"]}
      emptyLabel={t(($) => $.pickers.executor.trigger_unassigned)}
      searchPlaceholder={t(($) => $.pickers.executor.search_placeholder)}
      onSelect={(type, id) =>
        onUpdate({
          executor_type: type === "agent" || type === "team" ? type : null,
          executor_id: type === "agent" || type === "team" ? id : null,
        })
      }
    />
  );
}

/** Review can be performed by either a person or a runnable agent/team. */
export function ReviewerPicker({ reviewerType, reviewerId, onUpdate, ...props }: ReviewerPickerProps) {
  const { t } = useT("issues");
  return (
    <ActorRolePicker
      {...props}
      actorType={reviewerType}
      actorId={reviewerId}
      allowedKinds={["member", "agent", "team"]}
      emptyLabel={t(($) => $.pickers.reviewer.trigger_unassigned)}
      searchPlaceholder={t(($) => $.pickers.reviewer.search_placeholder)}
      onSelect={(type, id) => onUpdate({ reviewer_type: type, reviewer_id: id })}
    />
  );
}

function ActorRolePicker(props: ActorRolePickerProps) {
  const hasDeferredTrigger =
    props.trigger !== undefined || props.triggerRender?.props.children != null;
  const canDefer =
    props.open === undefined && props.onOpenChange === undefined && hasDeferredTrigger;
  if (!canDefer) return <ActorRolePickerImpl {...props} />;

  return (
    <DeferredPopup
      trigger={props.trigger}
      triggerRender={props.triggerRender}
      triggerClassName={PICKER_TRIGGER_CLASS}
    >
      {(open, onOpenChange) => (
        <ActorRolePickerImpl {...props} open={open} onOpenChange={onOpenChange} />
      )}
    </DeferredPopup>
  );
}

function ActorRolePickerImpl({
  actorType,
  actorId,
  allowedKinds,
  onSelect,
  emptyLabel,
  searchPlaceholder,
  mixed = false,
  trigger: customTrigger,
  triggerRender,
  open: controlledOpen,
  onOpenChange: controlledOnOpenChange,
  align,
  allowUnassigned = true,
}: ActorRolePickerProps) {
  const { t } = useT("issues");
  const [internalOpen, setInternalOpen] = useState(false);
  const open = controlledOpen ?? internalOpen;
  const setOpen = controlledOnOpenChange ?? setInternalOpen;
  const [filter, setFilter] = useState("");
  const user = useAuthStore((state) => state.user);
  const workspaceId = useWorkspaceId();
  const allowed = useMemo(() => new Set(allowedKinds), [allowedKinds]);
  const { data: members = [] } = useQuery({
    ...memberListOptions(workspaceId),
    enabled: allowed.has("member"),
  });
  const { data: agents = [] } = useQuery({
    ...agentListOptions(workspaceId),
    enabled: allowed.has("agent"),
  });
  const { data: teams = [] } = useQuery({
    ...teamListOptions(workspaceId),
    enabled: allowed.has("team"),
  });
  const { data: frequency = [] } = useQuery({
    ...executorFrequencyOptions(workspaceId),
    enabled: allowed.has("agent") || allowed.has("team"),
  });
  const { getActorName } = useActorName();

  const currentMember = members.find((member) => member.user_id === user?.id);
  const memberRole = currentMember?.role;
  const frequencyByActor = useMemo(
    () =>
      new Map(
        frequency.map((entry) => [
          `${entry.executor_type}:${entry.executor_id}`,
          entry.frequency,
        ]),
      ),
    [frequency],
  );
  const frequencyOf = (type: IssueActorType, id: string) =>
    frequencyByActor.get(`${type}:${id}`) ?? 0;
  const query = filter.trim().toLowerCase();
  const matches = (name: string) =>
    name.toLowerCase().includes(query) || matchesPinyin(name, query);
  const filteredMembers = allowed.has("member")
    ? members.filter((member) => matches(member.name))
    : [];
  const filteredAgents = allowed.has("agent")
    ? agents
        .filter((agent) => !agent.archived_at && matches(agent.name))
        .sort((left, right) =>
          frequencyOf("agent", right.id) - frequencyOf("agent", left.id),
        )
    : [];
  const filteredTeams = allowed.has("team")
    ? teams
        .filter((team) => !team.archived_at && matches(team.name))
        .sort((left, right) =>
          frequencyOf("team", right.id) - frequencyOf("team", left.id),
        )
    : [];
  const runnableAgentIds = new Set(
    agents
      .filter((agent) => !agent.archived_at && isAgentRuntimeBound(agent))
      .map((agent) => agent.id),
  );
  const selected = (type: IssueActorType, id: string) =>
    actorType === type && actorId === id;

  return (
    <PropertyPicker
      open={open}
      onOpenChange={(nextOpen: boolean) => {
        setOpen(nextOpen);
        if (!nextOpen) setFilter("");
      }}
      width="w-64"
      align={align}
      searchable
      searchPlaceholder={searchPlaceholder}
      onSearchChange={setFilter}
      triggerRender={triggerRender}
      trigger={
        customTrigger ? customTrigger : actorType && actorId ? (
          <>
            <ActorAvatar
              actorType={actorType}
              actorId={actorId}
              size="sm"
              enableHoverCard
              showStatusDot={actorType === "agent"}
            />
            <span className="truncate">{getActorName(actorType, actorId)}</span>
          </>
        ) : (
          <span className="text-muted-foreground">{emptyLabel}</span>
        )
      }
    >
      {allowUnassigned ? (
        <PickerItem
          emptyValue
          selected={!mixed && !actorType && !actorId}
          onClick={() => {
            onSelect(null, null);
            setOpen(false);
          }}
        >
          <UserMinus className="h-3.5 w-3.5 text-muted-foreground" />
          <span className="text-muted-foreground">{emptyLabel}</span>
        </PickerItem>
      ) : null}

      {filteredMembers.length > 0 ? (
        <PickerSection label={t(($) => $.pickers.executor.members_group)}>
          {filteredMembers.map((member) => (
            <PickerItem
              key={member.user_id}
              selected={selected("member", member.user_id)}
              onClick={() => {
                onSelect("member", member.user_id);
                setOpen(false);
              }}
            >
              <ActorAvatar actorType="member" actorId={member.user_id} size="sm" />
              <span className="truncate">{member.name}</span>
            </PickerItem>
          ))}
        </PickerSection>
      ) : null}

      {filteredAgents.length > 0 ? (
        <PickerSection label={t(($) => $.pickers.executor.agents_group)}>
          {filteredAgents.map((agent) => {
            const decision = canAssignAgentToIssue(agent, {
              userId: user?.id ?? null,
              role:
                memberRole === "owner" ||
                memberRole === "admin" ||
                memberRole === "member"
                  ? memberRole
                  : null,
            });
            const runtimeBound = isAgentRuntimeBound(agent);
            const enabled = decision.allowed && runtimeBound;
            return (
              <PickerItem
                key={agent.id}
                selected={selected("agent", agent.id)}
                disabled={!enabled}
                tooltip={
                  !decision.allowed
                    ? decision.message
                    : !runtimeBound
                      ? t(($) => $.pickers.executor.agent_runtime_required)
                      : undefined
                }
                onClick={() => {
                  if (!enabled) return;
                  onSelect("agent", agent.id);
                  setOpen(false);
                }}
              >
                <ActorAvatar actorType="agent" actorId={agent.id} size="sm" showStatusDot />
                <span className={`truncate ${enabled ? "" : "text-muted-foreground"}`}>
                  {agent.name}
                </span>
                {agent.visibility === "private" ? (
                  <Lock className="ml-auto h-3 w-3 text-muted-foreground" />
                ) : null}
              </PickerItem>
            );
          })}
        </PickerSection>
      ) : null}

      {filteredTeams.length > 0 ? (
        <PickerSection label={t(($) => $.pickers.executor.teams_group)}>
          {filteredTeams.map((team) => {
            const runtimeBound = runnableAgentIds.has(team.leader_id);
            return (
              <PickerItem
                key={team.id}
                selected={selected("team", team.id)}
                disabled={!runtimeBound}
                tooltip={
                  runtimeBound
                    ? undefined
                    : t(($) => $.pickers.executor.team_runtime_required)
                }
                onClick={() => {
                  if (!runtimeBound) return;
                  onSelect("team", team.id);
                  setOpen(false);
                }}
              >
                <ActorAvatar actorType="team" actorId={team.id} size="sm" />
                <span className="truncate">{team.name}</span>
              </PickerItem>
            );
          })}
        </PickerSection>
      ) : null}

      {filteredMembers.length === 0 &&
      filteredAgents.length === 0 &&
      filteredTeams.length === 0 &&
      filter ? (
        <PickerEmpty />
      ) : null}
    </PropertyPicker>
  );
}
