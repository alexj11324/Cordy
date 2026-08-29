"use client";

import { useQuery } from "@tanstack/react-query";
import type { TeamMemberPreview } from "@patchbay/core/types";
import { useWorkspaceId } from "@patchbay/core/hooks";
import {
  teamListOptions,
  agentListOptions,
  memberListOptions,
} from "@patchbay/core/workspace/queries";
import { useWorkspacePaths } from "@patchbay/core/paths";
import { ActorAvatar as ActorAvatarBase } from "@patchbay/ui/components/common/actor-avatar";
import { Skeleton } from "@patchbay/ui/components/ui/skeleton";
import { ActorAvatar } from "../../common/actor-avatar";
import { AppLink } from "../../navigation";
import { useT } from "../../i18n";

interface TeamProfileCardProps {
  teamId: string;
}

export function TeamProfileCard({ teamId }: TeamProfileCardProps) {
  const { t } = useT("teams");
  const wsId = useWorkspaceId();
  const p = useWorkspacePaths();
  const { data: teams = [], isLoading: teamsLoading } = useQuery(
    teamListOptions(wsId),
  );
  const { data: agents = [] } = useQuery(agentListOptions(wsId));
  const { data: wsMembers = [] } = useQuery(memberListOptions(wsId));

  const team = teams.find((s) => s.id === teamId);

  if (teamsLoading && !team) {
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

  if (!team) {
    return (
      <div className="text-caption text-muted-foreground">
        {t(($) => $.profile_card.unavailable)}
      </div>
    );
  }

  const isArchived = !!team.archived_at;
  const initials = team.name
    .split(" ")
    .map((w) => w[0])
    .join("")
    .toUpperCase()
    .slice(0, 2);

  const memberPreview = team.member_preview ?? [];
  const memberCount = team.member_count ?? memberPreview.length;

  return (
    <div className="group flex flex-col gap-3 text-left">
      <div className="flex items-start gap-3">
        <ActorAvatarBase
          name={team.name}
          initials={initials}
          avatarUrl={team.avatar_url}
          isTeam
          size="xl"
        />
        <div className="min-w-0 flex-1">
          <div className="flex items-center gap-1.5">
            <p className="truncate text-body font-semibold">{team.name}</p>
            {isArchived && (
              <span className="rounded-md bg-muted px-1.5 py-0.5 text-micro font-medium text-muted-foreground">
                {t(($) => $.profile_card.archived)}
              </span>
            )}
          </div>
        </div>
        {!isArchived && (
          <AppLink
            href={p.teamDetail(team.id)}
            className="mr-1 mt-0.5 shrink-0 text-caption font-normal text-brand opacity-0 transition-opacity group-hover:opacity-100"
          >
            {t(($) => $.profile_card.detail_link)}
          </AppLink>
        )}
      </div>

      {team.description && (
        <p className="line-clamp-2 text-caption text-muted-foreground">
          {team.description}
        </p>
      )}

      {memberCount > 0 && (
        <MembersList
          members={memberPreview}
          memberCount={memberCount}
          leaderId={team.leader_id}
          agents={agents}
          wsMembers={wsMembers}
        />
      )}
    </div>
  );
}

function MembersList({
  members,
  memberCount,
  leaderId,
  agents,
  wsMembers,
}: {
  members: TeamMemberPreview[];
  memberCount: number;
  leaderId: string;
  agents: { id: string; name: string }[];
  wsMembers: { user_id: string; name: string; role: string }[];
}) {
  const { t } = useT("teams");
  const p = useWorkspacePaths();
  const visible = members.slice(0, 3);
  const overflow = Math.max(0, memberCount - visible.length);

  return (
    <div className="flex flex-col gap-1.5 text-caption">
      <span className="text-muted-foreground">
        {t(($) => $.profile_card.members_section)}
        <span className="ml-1 tabular-nums">· {memberCount}</span>
      </span>
      <div className="flex flex-col gap-0.5">
        {visible.map((m) => {
          const isLeader =
            m.member_type === "agent" && m.member_id === leaderId;
          const name =
            m.member_type === "agent"
              ? agents.find((a) => a.id === m.member_id)?.name ??
                m.member_id.slice(0, 8)
              : wsMembers.find((u) => u.user_id === m.member_id)?.name ??
                m.member_id.slice(0, 8);
          const href =
            m.member_type === "agent"
              ? p.agentDetail(m.member_id)
              : p.memberDetail(m.member_id);
          const memberRole =
            m.member_type === "member"
              ? wsMembers.find((u) => u.user_id === m.member_id)?.role ?? null
              : null;

          return (
            <AppLink
              key={`${m.member_type}-${m.member_id}`}
              href={href}
              className="flex min-w-0 items-center gap-2 rounded-md px-2 py-1.5 transition-colors hover:bg-accent/60"
            >
              <ActorAvatar
                actorType={m.member_type}
                actorId={m.member_id}
                size="sm"
                showStatusDot={m.member_type === "agent"}
                className="shrink-0"
              />
              <span className="min-w-0 flex-1 truncate font-medium">{name}</span>
              {isLeader && (
                <span className="max-w-[4rem] shrink-0 truncate rounded-md bg-amber-100 px-1 py-0.5 text-micro font-medium text-amber-700 dark:bg-amber-900/30 dark:text-amber-400">
                  {t(($) => $.members_tab.leader_chip)}
                </span>
              )}
              {m.member_type === "member" && memberRole && (
                <span className="max-w-[3.5rem] shrink-0 truncate text-muted-foreground">
                  {memberRole}
                </span>
              )}
            </AppLink>
          );
        })}
        {overflow > 0 && (
          <span className="px-2 py-0.5 text-muted-foreground">
            {t(($) => $.profile_card.more_members, { count: overflow })}
          </span>
        )}
      </div>
    </div>
  );
}
