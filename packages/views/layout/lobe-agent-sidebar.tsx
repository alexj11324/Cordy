"use client";

import { Flexbox } from "@lobehub/ui/es/Flex/index";
import SearchBar from "@lobehub/ui/es/SearchBar/index";
import { Button as LobeButton } from "@lobehub/ui/base-ui";
import { MotionProvider } from "@lobehub/ui/es/MotionProvider/index";
import type { Agent, ChatSession, Invitation, User, Workspace } from "@patchbay/core/types";
import {
  Check,
  ChevronDown,
  ChevronUp,
  ListTodo,
  MessageSquare,
  MessageSquarePlus,
  MoreHorizontal,
  Search,
  UserRound,
} from "lucide-react";
import { useMemo, useState, type CSSProperties, type ReactNode } from "react";
import { motion } from "motion/react";
import { cn } from "@patchbay/ui/lib/utils";
import {
  Sidebar,
  SidebarContent,
  SidebarFooter,
  SidebarHeader,
  SidebarMenu,
  SidebarMenuButton,
  SidebarMenuItem,
  SidebarRail,
} from "@patchbay/ui/components/ui/sidebar";
import { CappedNumberFlow } from "@patchbay/ui/components/ui/number-flow";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuGroup,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@patchbay/ui/components/ui/dropdown-menu";
import { ActorAvatar as BaseActorAvatar } from "@patchbay/ui/components/common/actor-avatar";
import { resolvePublicFileUrl } from "@patchbay/core/workspace/avatar-url";
import { paths } from "@patchbay/core/paths";
import { WorkspaceAvatar } from "../workspace/workspace-avatar";
import { AppLink } from "../navigation";
import { ActorAvatar as AgentActorAvatar } from "../common/actor-avatar";
import { HelpLauncher } from "./help-launcher";
import { AgentPicker } from "../chat/components/new-chat-button";
import { ChatThreadList } from "../chat/components/chat-thread-list";
import { useT } from "../i18n";

type LobeNavItemProps = {
  active?: boolean;
  href?: string;
  icon: ReactNode;
  label: string;
  onClick: () => void;
  trailing?: ReactNode;
};

function LobeNavItem({ active = false, href, icon, label, onClick, trailing }: LobeNavItemProps) {
  return (
    <button
      type="button"
      aria-current={active ? "page" : undefined}
      data-active={active ? "true" : undefined}
      data-href={href}
      onClick={onClick}
      className={cn(
        "group/nav flex h-9 w-full items-center gap-2 rounded-lg px-1 text-left text-body text-sidebar-text-secondary outline-none transition-colors",
        "hover:bg-sidebar-item-hover hover:text-sidebar-text-primary focus-visible:ring-2 focus-visible:ring-sidebar-ring",
        "data-[active=true]:bg-sidebar-item-active data-[active=true]:font-medium data-[active=true]:text-sidebar-item-active-foreground",
        "group-data-[collapsible=icon]:justify-center group-data-[collapsible=icon]:px-0",
      )}
    >
      <Flexbox
        align="center"
        justify="center"
        width={28}
        height={28}
        flex="none"
        className={cn(
          "text-sidebar-icon-secondary transition-colors group-hover/nav:text-sidebar-text-primary",
          active && "text-sidebar-icon-active",
        )}
      >
        {icon}
      </Flexbox>
      <span className="min-w-0 flex-1 truncate group-data-[collapsible=icon]:hidden">{label}</span>
      {trailing && <span className="shrink-0 group-data-[collapsible=icon]:hidden">{trailing}</span>}
    </button>
  );
}

export type LobeAgentSidebarProps = {
  activeAgent: Agent | null;
  activeSessionId: string | null;
  agents: Agent[];
  availableAgents: Agent[];
  chatHref: string;
  chatUnreadCount: number;
  headerClassName?: string;
  headerStyle?: CSSProperties;
  acceptingInvitation?: boolean;
  decliningInvitation?: boolean;
  myInvitations: Invitation[];
  onAcceptInvitation: (id: string) => void;
  onArchive: (session: ChatSession) => void;
  onDeclineInvitation: (id: string) => void;
  onOpenProfile: () => void;
  onOpenSearch: () => void;
  onOpenTasks: () => void;
  onOpenTopics: () => void;
  onSelectSession: (session: ChatSession) => void;
  onStartChat: (agent: Agent | null) => void;
  onSwitchWorkspace: (workspace: Workspace) => void;
  onCreateWorkspace: () => void;
  otherWorkspaceUnread: boolean;
  sidebarState: "expanded" | "collapsed";
  setHoverRevealSuspended: (suspended: boolean) => void;
  sessions: ChatSession[];
  topSlot?: ReactNode;
  unreadWorkspaceIds: Set<string>;
  user: User | null | undefined;
  userId: string | undefined;
  workspace: Workspace | null | undefined;
  workspaces: Workspace[];
  workspaceCreationDisabled: boolean;
  onLogout: () => void;
};

function filterSessions(sessions: ChatSession[], agents: Agent[], query: string) {
  const normalized = query.trim().toLocaleLowerCase();
  if (!normalized) return sessions;

  const agentNames = new Map(agents.map((agent) => [agent.id, agent.name.toLocaleLowerCase()]));
  return sessions.filter((session) => {
    const lastMessage = session.last_message?.content.toLocaleLowerCase() ?? "";
    const title = session.title.toLocaleLowerCase();
    const agentName = agentNames.get(session.agent_id) ?? "";
    return [title, agentName, lastMessage].some((value) => value.includes(normalized));
  });
}

export function LobeAgentSidebar({
  activeAgent,
  activeSessionId,
  agents,
  availableAgents,
  chatHref,
  chatUnreadCount,
  headerClassName,
  headerStyle,
  acceptingInvitation = false,
  decliningInvitation = false,
  myInvitations,
  onAcceptInvitation,
  onArchive,
  onDeclineInvitation,
  onOpenProfile,
  onOpenSearch,
  onOpenTasks,
  onOpenTopics,
  onSelectSession,
  onStartChat,
  onSwitchWorkspace,
  onCreateWorkspace,
  otherWorkspaceUnread,
  sidebarState,
  setHoverRevealSuspended,
  sessions,
  topSlot,
  unreadWorkspaceIds,
  user,
  userId,
  workspace,
  workspaces,
  workspaceCreationDisabled,
  onLogout,
}: LobeAgentSidebarProps) {
  const { t: chatT } = useT("chat");
  const { t: layoutT } = useT("layout");
  const [sessionFilter, setSessionFilter] = useState("");
  const filteredSessions = useMemo(
    () => filterSessions(sessions, agents, sessionFilter),
    [agents, sessions, sessionFilter],
  );
  const historyCount = sessions.filter((session) => session.status !== "archived").length;

  const workspaceTrigger = (
    <SidebarMenuButton
      size="lg"
      className="min-w-0 rounded-lg px-1.5 group-data-[collapsible=icon]:justify-center group-data-[collapsible=icon]:px-0"
    >
      <span className="relative shrink-0">
        <WorkspaceAvatar name={workspace?.name ?? "M"} avatarUrl={workspace?.avatar_url} size="md" />
        {(myInvitations.length > 0 || otherWorkspaceUnread) && (
          <span className="absolute -right-0.5 -top-0.5 size-2 rounded-full bg-brand ring-2 ring-sidebar" />
        )}
      </span>
      <span className="min-w-0 flex-1 truncate text-left text-body font-medium group-data-[collapsible=icon]:hidden">
        {workspace?.name ?? "Patchbay"}
      </span>
      <ChevronDown className="size-3.5 shrink-0 text-sidebar-icon-secondary group-data-[collapsible=icon]:hidden" />
    </SidebarMenuButton>
  );

  const agentTrigger = (
    <SidebarMenuButton
      size="lg"
      aria-label={chatT(($) => $.navigation.switch_agent)}
      className="min-w-0 rounded-lg px-1.5 group-data-[collapsible=icon]:justify-center group-data-[collapsible=icon]:px-0"
    />
  );

  return (
    <Sidebar variant="inset">
      {topSlot}
      <SidebarHeader className={cn("gap-1.5 px-3 pb-2 pt-3", headerClassName)} style={headerStyle}>
        <SidebarMenu>
          <SidebarMenuItem>
            <DropdownMenu onOpenChange={setHoverRevealSuspended}>
              <DropdownMenuTrigger render={workspaceTrigger} />
              <DropdownMenuContent className="min-w-56" align="start" side="bottom" sideOffset={4}>
                <DropdownMenuGroup>
                  <DropdownMenuLabel className="text-caption text-sidebar-text-secondary">
                    {chatT(($) => $.navigation.switch_workspace)}
                  </DropdownMenuLabel>
                  {workspaces.map((item) => (
                    <DropdownMenuItem
                      key={item.id}
                      render={<AppLink href={paths.workspace(item.slug).issues()} />}
                      onClick={() => onSwitchWorkspace(item)}
                    >
                      <WorkspaceAvatar name={item.name} avatarUrl={item.avatar_url} size="sm" />
                      <span className="min-w-0 flex-1 truncate">{item.name}</span>
                      {item.id !== workspace?.id && unreadWorkspaceIds.has(item.id) && (
                        <span className="size-2 rounded-full bg-brand" />
                      )}
                      {item.id === workspace?.id && <Check className="size-3.5 text-primary" />}
                    </DropdownMenuItem>
                  ))}
                  {!workspaceCreationDisabled && (
                    <DropdownMenuItem onClick={onCreateWorkspace}>
                      <MessageSquarePlus className="size-3.5" />
                    {layoutT(($) => $.sidebar.create_workspace)}
                    </DropdownMenuItem>
                  )}
                </DropdownMenuGroup>
                {myInvitations.length > 0 && (
                  <>
                    <DropdownMenuSeparator />
                    <DropdownMenuGroup>
                      <DropdownMenuLabel className="text-caption text-sidebar-text-secondary">
                    {layoutT(($) => $.sidebar.pending_invitations_label)}
                      </DropdownMenuLabel>
                      {myInvitations.map((invitation) => (
                        <div key={invitation.id} className="flex items-center gap-2 px-2 py-1.5">
                          <WorkspaceAvatar name={invitation.workspace_name ?? "W"} size="sm" />
                          <span className="min-w-0 flex-1 truncate text-body">
                            {invitation.workspace_name ?? layoutT(($) => $.sidebar.invitation_workspace_fallback)}
                          </span>
                          <button
                            type="button"
                            className="rounded bg-primary px-2 py-0.5 text-caption text-primary-foreground hover:bg-primary/90 disabled:opacity-50"
                            disabled={acceptingInvitation}
                            onClick={() => onAcceptInvitation(invitation.id)}
                          >
                            {layoutT(($) => $.sidebar.invitation_join)}
                          </button>
                          <button
                            type="button"
                            className="rounded bg-muted px-2 py-0.5 text-caption text-sidebar-text-secondary hover:bg-muted/80 disabled:opacity-50"
                            disabled={decliningInvitation}
                            onClick={() => onDeclineInvitation(invitation.id)}
                          >
                            {layoutT(($) => $.sidebar.invitation_decline)}
                          </button>
                        </div>
                      ))}
                    </DropdownMenuGroup>
                  </>
                )}
              </DropdownMenuContent>
            </DropdownMenu>
          </SidebarMenuItem>
          <SidebarMenuItem>
            <AgentPicker
              agents={availableAgents}
              userId={userId}
              currentAgentId={activeAgent?.id}
              onSelect={(agent) => onStartChat(agent)}
              triggerRender={agentTrigger}
              trigger={
                <>
                  {activeAgent ? (
                    <AgentActorAvatar
                      actorType="agent"
                      actorId={activeAgent.id}
                      size="md"
                      profileLink={false}
                      showStatusDot
                    />
                  ) : (
                    <span className="inline-flex size-6 shrink-0 items-center justify-center rounded-full bg-sidebar-item-active text-sidebar-icon-secondary">
                      <MessageSquare className="size-3.5" />
                    </span>
                  )}
                  <span className="min-w-0 flex-1 truncate text-left text-body font-medium group-data-[collapsible=icon]:hidden">
                    {activeAgent?.name ?? chatT(($) => $.window.no_agents)}
                  </span>
                  <ChevronDown className="size-3.5 shrink-0 text-sidebar-icon-secondary group-data-[collapsible=icon]:hidden" />
                </>
              }
            />
          </SidebarMenuItem>
        </SidebarMenu>

        <div className="pt-1 group-data-[collapsible=icon]:px-0">
          <MotionProvider motion={motion}>
            <LobeButton
              block
              type="text"
              size="middle"
              icon={<MessageSquarePlus className="size-4" />}
              aria-label={chatT(($) => $.navigation.new_topic)}
              onClick={() => onStartChat(activeAgent)}
              className="!h-9 !justify-start !rounded-lg !px-2 !text-sidebar-text-primary hover:!bg-sidebar-item-hover group-data-[collapsible=icon]:!justify-center group-data-[collapsible=icon]:!px-0"
            >
              <span className="group-data-[collapsible=icon]:hidden">{chatT(($) => $.navigation.new_topic)}</span>
            </LobeButton>
          </MotionProvider>
        </div>
      </SidebarHeader>

      <SidebarContent className="min-h-0 px-2" style={{ scrollbarGutter: "stable" }}>
        <div className="space-y-0.5 px-1 pt-1">
          <LobeNavItem
            icon={<Search className="size-[18px]" />}
            label={chatT(($) => $.navigation.search)}
            onClick={onOpenSearch}
          />
          <LobeNavItem
            active
            icon={<MessageSquare className="size-[18px]" />}
            href={chatHref}
            label={chatT(($) => $.navigation.topics)}
            onClick={onOpenTopics}
            trailing={
              chatUnreadCount > 0 ? (
                <CappedNumberFlow
                  value={chatUnreadCount}
                  animated={false}
                  className="text-caption"
                />
              ) : undefined
            }
          />
          <LobeNavItem
            icon={<UserRound className="size-[18px]" />}
            label={chatT(($) => $.navigation.profile)}
            onClick={onOpenProfile}
          />
          <LobeNavItem
            icon={<ListTodo className="size-[18px]" />}
            label={chatT(($) => $.navigation.tasks)}
            onClick={onOpenTasks}
          />
        </div>

        <div className="mt-3 flex min-h-0 flex-1 flex-col group-data-[collapsible=icon]:hidden">
          <div className="flex h-8 shrink-0 items-center gap-1 px-1">
            <span className="min-w-0 flex-1 truncate text-micro font-medium text-sidebar-text-tertiary group-data-[collapsible=icon]:hidden">
              {chatT(($) => $.navigation.history)}
            </span>
            <span className="text-micro text-sidebar-text-tertiary group-data-[collapsible=icon]:hidden">
              {chatT(($) => $.navigation.topic_count, { count: historyCount })}
            </span>
            <button
              type="button"
              aria-label={chatT(($) => $.navigation.topic_actions)}
              className="inline-flex size-7 shrink-0 items-center justify-center rounded-md text-sidebar-icon-secondary outline-none hover:bg-sidebar-item-hover hover:text-sidebar-text-primary focus-visible:ring-2 focus-visible:ring-sidebar-ring group-data-[collapsible=icon]:hidden"
            >
              <MoreHorizontal className="size-4" />
            </button>
          </div>
          <div className="shrink-0 px-1 pb-2 group-data-[collapsible=icon]:hidden">
            <SearchBar
              value={sessionFilter}
              onChange={(event) => setSessionFilter(event.target.value)}
              placeholder={chatT(($) => $.navigation.history_search)}
              className="w-full"
              aria-label={chatT(($) => $.navigation.history_search)}
            />
          </div>
          <div className="min-h-0 flex-1 overflow-y-auto pb-2">
            <ChatThreadList
              sessions={filteredSessions}
              agents={agents}
              activeSessionId={activeSessionId}
              onSelectSession={onSelectSession}
              onArchive={onArchive}
            />
          </div>
        </div>
      </SidebarContent>

      <SidebarFooter className="p-2">
        <SidebarMenu>
          <SidebarMenuItem>
            <div className="flex min-w-0 items-center gap-1">
              <DropdownMenu onOpenChange={setHoverRevealSuspended}>
                <DropdownMenuTrigger
                  render={
                    <SidebarMenuButton size="lg" className="min-w-0 flex-1 rounded-lg px-1.5 text-left">
                      <BaseActorAvatar
                        name={user?.name ?? ""}
                        initials={(user?.name ?? "U").charAt(0).toUpperCase()}
                        avatarUrl={resolvePublicFileUrl(user?.avatar_url)}
                        size="sm"
                      />
                      <span className="min-w-0 flex-1 group-data-[collapsible=icon]:hidden">
                        <span className="block truncate text-body font-medium leading-tight">{user?.name}</span>
                        <span className="block truncate text-caption text-sidebar-text-secondary leading-tight">{user?.email}</span>
                      </span>
                      <ChevronUp className="ml-auto size-4 text-sidebar-icon-secondary group-data-[collapsible=icon]:hidden" />
                    </SidebarMenuButton>
                  }
                />
                <DropdownMenuContent side="top" align="start" sideOffset={8} className="min-w-56">
                  <div className="flex items-center gap-2.5 px-2 py-1.5">
                    <BaseActorAvatar
                      name={user?.name ?? ""}
                      initials={(user?.name ?? "U").charAt(0).toUpperCase()}
                      avatarUrl={resolvePublicFileUrl(user?.avatar_url)}
                      size="lg"
                    />
                    <div className="min-w-0 flex-1">
                      <p className="truncate text-body font-medium leading-tight">{user?.name}</p>
                      <p className="truncate text-caption text-sidebar-text-secondary leading-tight">{user?.email}</p>
                    </div>
                  </div>
                  <DropdownMenuSeparator />
                  <DropdownMenuItem onClick={onLogout}>
                    {layoutT(($) => $.sidebar.log_out)}
                  </DropdownMenuItem>
                </DropdownMenuContent>
              </DropdownMenu>
              {sidebarState !== "collapsed" && <HelpLauncher onOpenChange={setHoverRevealSuspended} />}
            </div>
          </SidebarMenuItem>
        </SidebarMenu>
      </SidebarFooter>
      <SidebarRail />
    </Sidebar>
  );
}
