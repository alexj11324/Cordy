"use client";

import { issueStatusCategory } from "@patchbay/core/issues";
import React, { useCallback, useEffect, useRef, useState } from "react";
import { cn } from "@patchbay/ui/lib/utils";
import { useScrollFade } from "@patchbay/ui/hooks/use-scroll-fade";
import { AppLink, useNavigation } from "../navigation";
import { HelpLauncher } from "./help-launcher";
import {
  DndContext,
  PointerSensor,
  useSensor,
  useSensors,
  closestCenter,
  type DragEndEvent,
} from "@dnd-kit/core";
import { SortableContext, verticalListSortingStrategy, useSortable, arrayMove } from "@dnd-kit/sortable";
import { CSS } from "@dnd-kit/utilities";
import { Layers,
  ChevronDown,
  ChevronUp,
  ChevronRight,
  LogOut,
  Plus,
  Check,
  SquarePen,
  X,
} from "lucide-react";
import { WorkspaceAvatar } from "../workspace/workspace-avatar";
import { ActorAvatar } from "@patchbay/ui/components/common/actor-avatar";
import { Tooltip, TooltipTrigger, TooltipContent } from "@patchbay/ui/components/ui/tooltip";
import { Collapsible, CollapsibleTrigger, CollapsibleContent } from "@patchbay/ui/components/ui/collapsible";
import { CappedNumberFlow } from "@patchbay/ui/components/ui/number-flow";
import { StatusIcon } from "../issues/components/status-icon";
import { useIssueDraftStore } from "@patchbay/core/issues/stores/draft-store";
import { openCreateIssueWithPreference } from "@patchbay/core/issues/stores/create-mode-store";
import {
  Sidebar,
  SidebarContent,
  SidebarFooter,
  SidebarGroup,
  SidebarGroupContent,
  SidebarGroupLabel,
  SidebarHeader,
  SidebarMenu,
  SidebarMenuButton,
  SidebarMenuItem,
  SidebarRail,
  useSidebar,
} from "@patchbay/ui/components/ui/sidebar";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuGroup,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@patchbay/ui/components/ui/dropdown-menu";
import { useAuthStore } from "@patchbay/core/auth";
import { issueViewDetailOptions } from "@patchbay/core/issue-views/queries";
import {
  issueViewContainerKey,
  useActiveIssueViewStore,
} from "@patchbay/core/issue-views/active-view-store";
import { useCurrentWorkspace, useWorkspacePaths, paths } from "@patchbay/core/paths";
import {
  agentListOptions,
  memberListOptions,
  workspaceListOptions,
  myInvitationListOptions,
  workspaceKeys,
} from "@patchbay/core/workspace/queries";
import { resolvePublicFileUrl } from "@patchbay/core/workspace/avatar-url";
import { useQuery, useMutation, useQueryClient } from "@tanstack/react-query";
import { inboxKeys, deduplicateInboxItems, inboxUnreadSummaryOptions, hasOtherWorkspaceUnread, unreadWorkspaceIds } from "@patchbay/core/inbox/queries";
import { chatSessionsOptions, sortChatSessions } from "@patchbay/core/chat/queries";
import { countUnreadChatMessages } from "@patchbay/core/chat/unread";
import { useChatStore } from "@patchbay/core/chat";
import { useSetChatSessionArchived } from "@patchbay/core/chat/mutations";
import { api, ApiError } from "@patchbay/core/api";
import { useConfigStore } from "@patchbay/core/config";
import { pinListOptions } from "@patchbay/core/pins/queries";
import { useDeletePin, useReorderPins } from "@patchbay/core/pins/mutations";
import { issueDetailOptions } from "@patchbay/core/issues/queries";
import { projectDetailOptions } from "@patchbay/core/projects/queries";
import type { Agent, ChatSession, PinnedItem } from "@patchbay/core/types";
import { useLogout } from "../auth";
import { ProjectIcon } from "../projects/components/project-icon";
import { routeIconForPath } from "./route-icon-components";
import { useT } from "../i18n";
import {
  useShortcut,
} from "@patchbay/core/shortcuts";
import { ShortcutKeycaps } from "../common/shortcut-keycaps";
import { useAppForeground } from "../common/use-app-foreground";
import { canAssignAgent } from "../issues/components/pickers/assignee-picker";
import { LobeAgentSidebar } from "./lobe-agent-sidebar";
import { useSearchStore } from "../search/search-store";

// Top-level nav items stay active when the user is on a child route
// (e.g. "Projects" stays lit on /:slug/projects/:id). Pinned items keep
// strict equality elsewhere — a pinned project shouldn't highlight on
// sub-pages of itself.
function isNavActive(pathname: string, href: string): boolean {
  return pathname === href || pathname.startsWith(href + "/");
}

// Stable empty arrays for query defaults. Using an inline `= []` default on
// `useQuery` creates a new array reference on every render when `data` is
// undefined (e.g. query disabled or loading) — which in turn breaks any
// `useEffect`/`useMemo` that depends on the value, and can trigger infinite
// re-render loops when the effect itself calls `setState`.
const EMPTY_PINS: PinnedItem[] = [];
const EMPTY_WORKSPACES: Awaited<ReturnType<typeof api.listWorkspaces>> = [];
const EMPTY_INVITATIONS: Awaited<ReturnType<typeof api.listMyInvitations>> = [];
const EMPTY_INBOX: Awaited<ReturnType<typeof api.listInbox>> = [];
const EMPTY_INBOX_SUMMARY: Awaited<ReturnType<typeof api.getInboxUnreadSummary>> = [];
const EMPTY_AGENTS: Agent[] = [];
const EMPTY_MEMBERS: Awaited<ReturnType<typeof api.listMembers>> = [];

// Nav items reference WorkspacePaths method names so they can be resolved
// against the current workspace slug at render time (see AppSidebar body).
// Only parameterless paths are valid nav destinations.
type NavKey =
  | "inbox"
  | "chat"
  | "channels"
  | "myIssues"
  | "issues"
  | "projects"
  | "automations"
  | "agents"
  | "teams"
  | "usage"
  | "runtimes"
  | "skills"
  | "integrations"
  | "settings";

// Static schema (key only) — labels resolved at render via useT("layout"),
// icons derived from the destination path via routeIconForPath.
type NavLabelKey =
  | "inbox"
  | "chat"
  | "channels"
  | "my_issues"
  | "issues"
  | "projects"
  | "automations"
  | "agents"
  | "teams"
  | "usage"
  | "runtimes"
  | "skills"
  | "integrations"
  | "settings";

// Nav icons are NOT declared here: they are derived from each item's
// destination path at render time, so the sidebar and the desktop tab bar
// always agree. See route-icon-components.tsx.
const personalNav: { key: NavKey; labelKey: NavLabelKey }[] = [
  { key: "inbox", labelKey: "inbox" },
  { key: "chat", labelKey: "chat" },
  { key: "channels", labelKey: "channels" },
  { key: "myIssues", labelKey: "my_issues" },
];

const workspaceNav: { key: NavKey; labelKey: NavLabelKey }[] = [
  { key: "issues", labelKey: "issues" },
  { key: "projects", labelKey: "projects" },
  { key: "automations", labelKey: "automations" },
  { key: "agents", labelKey: "agents" },
  { key: "integrations", labelKey: "integrations" },
  { key: "teams", labelKey: "teams" },
  { key: "usage", labelKey: "usage" },
];

const configureNav: { key: NavKey; labelKey: NavLabelKey }[] = [
  { key: "runtimes", labelKey: "runtimes" },
  { key: "skills", labelKey: "skills" },
  { key: "settings", labelKey: "settings" },
];

function sidebarNavIconClassName(isActive: boolean) {
  return cn("text-sidebar-icon-secondary", isActive && "text-sidebar-icon-active");
}

function DraftDot() {
  const hasDraft = useIssueDraftStore((s) => s.hasDraft());
  if (!hasDraft) return null;
  return <span className="absolute top-0 right-0 size-1.5 rounded-full bg-brand" />;
}

/**
 * Presentational pin row. The `label` and `iconNode` are computed by the
 * parent `PinRow` from cached issue / project detail queries — keeping
 * this component dumb means the dnd-kit / navigation wiring lives in
 * one place and the data flow is explicit.
 */
function SortablePinItem({
  pin,
  href,
  pathname,
  onUnpin,
  label,
  iconNode,
  onNavigate,
  isActiveOverride,
}: {
  pin: PinnedItem;
  href: string;
  pathname: string;
  onUnpin: () => void;
  label: string;
  iconNode: React.ReactNode;
  /** Runs on a real click (not a drag-release) before navigation. */
  onNavigate?: () => void;
  /** Overrides the plain path comparison (view pins carry extra state). */
  isActiveOverride?: boolean;
}) {
  const { t } = useT("layout");
  const { attributes, listeners, setNodeRef, transform, transition, isDragging } = useSortable({ id: pin.id });
  const wasDragged = useRef(false);

  useEffect(() => {
    if (isDragging) wasDragged.current = true;
  }, [isDragging]);

  const style = { transform: CSS.Transform.toString(transform), transition };
  const isActive = isActiveOverride ?? pathname === href;

  return (
    <SidebarMenuItem
      ref={setNodeRef}
      style={style}
      className={cn("group/pin", isDragging && "opacity-30")}
      {...attributes}
      {...listeners}
    >
      <SidebarMenuButton
        size="sm"
        isActive={isActive}
        render={<AppLink href={href} newTabTitle={label} draggable={false} />}
        onClick={(event) => {
          if (wasDragged.current) {
            wasDragged.current = false;
            event.preventDefault();
            return;
          }
          onNavigate?.();
        }}
        className={cn(
          "text-sidebar-text-secondary hover:not-data-active:bg-sidebar-item-hover data-active:bg-sidebar-item-active data-active:text-sidebar-item-active-foreground",
          isDragging && "pointer-events-none",
        )}
      >
        {iconNode}
        <span
          className="min-w-0 flex-1 overflow-hidden whitespace-nowrap"
          style={{
            maskImage: "linear-gradient(to right, black calc(100% - 12px), transparent)",
            WebkitMaskImage: "linear-gradient(to right, black calc(100% - 12px), transparent)",
          }}
        >{label}</span>
        <Tooltip>
          <TooltipTrigger
            render={<span role="button" />}
            className="hidden size-2.5 shrink-0 items-center justify-center rounded-sm text-sidebar-text-secondary group-hover/pin:flex hover:text-sidebar-text-primary"
            onClick={(event) => {
              event.preventDefault();
              event.stopPropagation();
              onUnpin();
            }}
          >
            <X className="size-1" />
          </TooltipTrigger>
          <TooltipContent side="top" sideOffset={4}>{t(($) => $.sidebar.unpin_tooltip)}</TooltipContent>
        </Tooltip>
      </SidebarMenuButton>
    </SidebarMenuItem>
  );
}

/**
 * Smart wrapper that resolves a pin's display data (label + status/icon)
 * from the issue / project detail query cache. Both queries are declared
 * unconditionally with `enabled` gates so the hook order stays stable
 * regardless of `pin.item_type`.
 *
 * Loading: render a flat skeleton so the sidebar height doesn't jump.
 * Missing (deleted item / 404): render nothing — the row hides itself
 * until the user unpins manually or a server-side cascade catches up.
 */
function PinRow({
  pin,
  href,
  pathname,
  onUnpin,
  wsId,
}: {
  pin: PinnedItem;
  href: string;
  pathname: string;
  onUnpin: () => void;
  wsId: string;
}) {
  const isIssue = pin.item_type === "issue";
  const isView = pin.item_type === "view";
  const p = useWorkspacePaths();
  const setActiveView = useActiveIssueViewStore((s) => s.setActive);
  const issueQuery = useQuery({
    ...issueDetailOptions(wsId, pin.item_id),
    enabled: isIssue,
  });
  const projectQuery = useQuery({
    ...projectDetailOptions(wsId, pin.item_id),
    enabled: pin.item_type === "project",
  });
  const viewQuery = useQuery({
    ...issueViewDetailOptions(wsId, pin.item_id),
    enabled: isView,
  });

  const triggeredRef = useRef(false);
  useEffect(() => {
    // Views are exempt from 404-auto-unpin: an installed desktop client
    // talking to an older backend without the view endpoints sees 404 for
    // every view pin — auto-unpinning would permanently delete them all.
    // A deleted view's row simply hides instead.
    if (isView) return;
    const err = isIssue ? issueQuery.error : projectQuery.error;
    if (err instanceof ApiError && err.status === 404 && !triggeredRef.current) {
      triggeredRef.current = true;
      onUnpin();
    }
  }, [isIssue, isView, issueQuery.error, onUnpin, projectQuery.error]);

  const activeViewByContainer = useActiveIssueViewStore((s) => s.active);
  if (isView) {
    if (viewQuery.isPending) return <PinSkeleton />;
    if (viewQuery.isError || !viewQuery.data) return null;
    const view = viewQuery.data;
    // One resolved scope drives the path AND the container key so an
    // unrecognised scope_type from a newer backend degrades coherently.
    const scopeType: "workspace" | "my" | "project" =
      view.scope_type === "my"
        ? "my"
        : view.scope_type === "project" && view.scope_id
          ? "project"
          : "workspace";
    const viewPath =
      scopeType === "my"
        ? p.myIssues()
        : scopeType === "project"
          ? p.projectDetail(view.scope_id!)
          : p.issues();
    const containerKey = issueViewContainerKey(wsId, {
      scope_type: scopeType,
      scope_id: scopeType === "project" ? view.scope_id : null,
    });
    return (
      <SortablePinItem
        pin={pin}
        // ?view= keeps a web reload on the view for the surfaces that mount
        // the URL-sync hook (/issues, /my-issues). Project pages don't sync
        // yet — there the query is inert and reload falls back to the plain
        // page; click-through activation still works everywhere.
        href={`${viewPath}?view=${view.id}`}
        pathname={pathname}
        onUnpin={onUnpin}
        label={view.name}
        iconNode={<Layers className="!size-3.5 shrink-0 text-sidebar-icon-secondary" />}
        // Active only when this exact view is open on its surface — the
        // path alone also matches the plain tab.
        isActiveOverride={
          pathname === viewPath && activeViewByContainer[containerKey] === view.id
        }
        onNavigate={() => setActiveView(containerKey, view.id)}
      />
    );
  }

  if (isIssue) {
    if (issueQuery.isPending) return <PinSkeleton />;
    if (issueQuery.isError || !issueQuery.data) return null;
    const issue = issueQuery.data;
    const label = issue.title;
    const iconNode = (
      /* Override parent [&_svg]:size-4 — pinned items need smaller icons to match sm size */
      <StatusIcon
        status={issue.status}
        category={issueStatusCategory(issue) ?? undefined}
        className="!size-3.5 shrink-0"
      />
    );
    return (
      <SortablePinItem
        pin={pin}
        href={href}
        pathname={pathname}
        onUnpin={onUnpin}
        label={label}
        iconNode={iconNode}
      />
    );
  }

  if (projectQuery.isPending) return <PinSkeleton />;
  if (projectQuery.isError || !projectQuery.data) return null;
  const project = projectQuery.data;
  const iconNode = <ProjectIcon project={project} size="sm" />;
  return (
    <SortablePinItem
      pin={pin}
      href={href}
      pathname={pathname}
      onUnpin={onUnpin}
      label={project.title}
      iconNode={iconNode}
    />
  );
}

function PinSkeleton() {
  return (
    <SidebarMenuItem>
      <div className="flex h-8 w-full items-center gap-2 px-1">
        <div className="size-3.5 shrink-0 rounded-sm bg-sidebar-accent/40" />
        <div className="h-3 w-24 rounded bg-sidebar-accent/40" />
      </div>
    </SidebarMenuItem>
  );
}

interface AppSidebarProps {
  /** Rendered above SidebarHeader (e.g. desktop traffic light spacer) */
  topSlot?: React.ReactNode;
  /** Rendered in the header between workspace switcher and new-issue button (e.g. search trigger) */
  searchSlot?: React.ReactNode;
  /** Extra className for SidebarHeader */
  headerClassName?: string;
  /** Extra style for SidebarHeader */
  headerStyle?: React.CSSProperties;
}

export function AppSidebar({ topSlot, searchSlot, headerClassName, headerStyle }: AppSidebarProps = {}) {
  const { t } = useT("layout");
  const { pathname, push } = useNavigation();
  const user = useAuthStore((s) => s.user);
  const userId = useAuthStore((s) => s.user?.id);
  const logout = useLogout();
  const workspace = useCurrentWorkspace();
  const p = useWorkspacePaths();
  const { data: workspaces = EMPTY_WORKSPACES } = useQuery(workspaceListOptions());
  const { data: myInvitations = EMPTY_INVITATIONS } = useQuery(myInvitationListOptions());
  const workspaceCreationDisabled = useConfigStore((s) => s.workspaceCreationDisabled);

  // On a phone the sidebar is a Sheet covering the page, so navigating out of
  // it has to dismiss it — otherwise the destination renders underneath and the
  // tap reads as "nothing happened". Closing on `pathname` rather than on each
  // link's onClick covers every route out of here at once: the nav groups, the
  // pinned items, the workspace switcher's programmatic push, and anything
  // added later. `setOpenMobile` is a no-op on desktop, where the sheet is not
  // the sidebar's rendering at all.
  const { isCompact, state: sidebarState, setOpenMobile, setHoverRevealSuspended } = useSidebar();
  useEffect(() => {
    setOpenMobile(false);
  }, [pathname, setOpenMobile]);

  const wsId = workspace?.id;
  const { data: inboxItems = EMPTY_INBOX } = useQuery({
    queryKey: wsId ? inboxKeys.list(wsId) : ["inbox", "disabled"],
    queryFn: () => api.listInbox(),
    enabled: !!wsId,
  });
  const unreadCount = React.useMemo(
    () => deduplicateInboxItems(inboxItems).filter((i) => !i.read).length,
    [inboxItems],
  );
  // Chat tab unread badge: IM-style total of unread *messages* across chat
  // threads (countUnreadChatMessages is the shared definition — mobile's tab
  // badge derives from the same function, keeping the platforms in agreement).
  const { data: chatSessions = [] } = useQuery({
    ...chatSessionsOptions(wsId ?? ""),
    enabled: !!wsId,
  });
  // The session the user is reading right now must not count: the thread list
  // renders its row badge as 0 (auto mark-read is about to clear it), and a
  // reply landing in the open conversation would otherwise flash a sidebar
  // count with no matching row. "Reading right now" = a session is active, a
  // chat surface is actually showing it (chat page route or the floating
  // window), AND the app is in the foreground. When the app is backgrounded,
  // auto mark-read is suppressed (PB-4485) so the reply stays unread — the
  // badge must count it, or the notification is silently eaten while the user
  // is away. A remembered selection while both surfaces are closed also still
  // counts, for the same reason.
  const activeChatSessionId = useChatStore((s) => s.activeSessionId);
  const floatingChatOpen = useChatStore((s) => s.isOpen);
  const appForeground = useAppForeground();
  const chatHref = p.chat();
  const isChatRoute = isNavActive(pathname, chatHref);
  const selectedChatAgentId = useChatStore((s) => s.selectedAgentId);
  const viewedChatSessionId =
    appForeground && (floatingChatOpen || isNavActive(pathname, chatHref))
      ? activeChatSessionId
      : null;
  const chatUnreadCount = React.useMemo(
    () => countUnreadChatMessages(chatSessions, viewedChatSessionId),
    [chatSessions, viewedChatSessionId],
  );
  // The Agent chat sidebar owns the agent picker, so fetch the same full agent
  // and member lists used by ChatPage only while that surface is active. The
  // member role is part of the permission decision; filtering by ownership
  // alone would show agents a workspace member cannot invoke.
  const { data: chatAgents = EMPTY_AGENTS } = useQuery({
    ...agentListOptions(wsId ?? ""),
    enabled: isChatRoute && !!wsId,
  });
  const { data: chatMembers = EMPTY_MEMBERS } = useQuery({
    ...memberListOptions(wsId ?? ""),
    enabled: isChatRoute && !!wsId,
  });
  const chatMemberRole = chatMembers.find((member) => member.user_id === userId)?.role;
  const availableChatAgents = React.useMemo(
    () => chatAgents.filter((agent) => !agent.archived_at && canAssignAgent(agent, userId, chatMemberRole)),
    [chatAgents, chatMemberRole, userId],
  );
  const activeChatSession = React.useMemo(
    () => chatSessions.find((session) => session.id === activeChatSessionId) ?? null,
    [activeChatSessionId, chatSessions],
  );
  const activeChatAgent = React.useMemo(() => {
    if (activeChatSession) {
      return chatAgents.find((agent) => agent.id === activeChatSession.agent_id) ?? null;
    }
    return availableChatAgents.find((agent) => agent.id === selectedChatAgentId) ?? availableChatAgents[0] ?? null;
  }, [activeChatSession, availableChatAgents, chatAgents, selectedChatAgentId]);
  // Cross-workspace unread summary backs the workspace-switcher dot. One
  // shared cache entry across workspaces; gated on an active workspace since
  // the endpoint resolves through the workspace-member middleware.
  const { data: unreadSummary = EMPTY_INBOX_SUMMARY } = useQuery({
    ...inboxUnreadSummaryOptions(),
    enabled: !!wsId,
  });
  const otherWorkspaceUnread = React.useMemo(
    () => hasOtherWorkspaceUnread(unreadSummary, wsId),
    [unreadSummary, wsId],
  );
  // Which workspaces have unread, so the switcher dropdown can point at the
  // specific one(s) rather than just the aggregate avatar dot.
  const unreadWsIds = React.useMemo(() => unreadWorkspaceIds(unreadSummary), [unreadSummary]);
  const { data: pinnedItems = EMPTY_PINS } = useQuery({
    ...pinListOptions(wsId ?? "", userId ?? ""),
    enabled: !!wsId && !!userId,
  });
  const deletePin = useDeletePin();
  const reorderPins = useReorderPins();
  const sensors = useSensors(useSensor(PointerSensor, { activationConstraint: { distance: 5 } }));
  const sidebarScrollRef = useRef<HTMLDivElement>(null);
  const sidebarFadeStyle = useScrollFade(sidebarScrollRef, 24);
  const getPinHref = useCallback(
    (pin: PinnedItem) =>
      pin.item_type === "issue"
        ? p.issueDetail(pin.item_id)
        : pin.item_type === "project"
          ? p.projectDetail(pin.item_id)
          // Views know their target only after their detail loads — the row
          // resolves its own href; this placeholder never renders as a link.
          : "",
    [p],
  );

  // Local presentational copy of pinnedItems for drop-animation stability.
  // Follows TQ at rest; frozen during a drag gesture so a mid-drag cache
  // write (our own optimistic update, or a WS refetch) cannot reorder the
  // DOM under dnd-kit while its drop animation is still interpolating.
  const [localPinned, setLocalPinned] = useState<PinnedItem[]>(pinnedItems);
  const [localPinnedWsId, setLocalPinnedWsId] = useState<string | null>(wsId ?? null);
  const isDraggingRef = useRef(false);
  useEffect(() => {
    if (!isDraggingRef.current) {
      setLocalPinned(pinnedItems);
    }
  }, [pinnedItems]);
  useEffect(() => {
    setLocalPinnedWsId(wsId ?? null);
  }, [wsId]);
  const visiblePinned = localPinnedWsId === (wsId ?? null) ? localPinned : EMPTY_PINS;
  // View pins are absent here (their href resolves async): while a view
  // pin is active the plain nav row for its surface stays highlighted too.
  // Accepted — suppressing it would need every view detail lifted up here.
  const isActivePinnedRoute = visiblePinned.some((pin) => pathname === getPinHref(pin));

  const handleDragStart = useCallback(() => {
    isDraggingRef.current = true;
  }, []);
  const handleDragEnd = useCallback(
    (event: DragEndEvent) => {
      isDraggingRef.current = false;
      const { active, over } = event;
      if (!over || active.id === over.id) return;
      const oldIndex = localPinned.findIndex((p) => p.id === active.id);
      const newIndex = localPinned.findIndex((p) => p.id === over.id);
      if (oldIndex === -1 || newIndex === -1) return;
      const reordered = arrayMove(localPinned, oldIndex, newIndex);
      setLocalPinned(reordered);
      reorderPins.mutate(reordered);
    },
    [localPinned, reorderPins],
  );

  const queryClient = useQueryClient();
  const acceptInvitationMut = useMutation({
    mutationFn: (id: string) => api.acceptInvitation(id),
    // After accepting an invitation, navigate INTO the newly-joined workspace.
    // Otherwise the user stays on their current workspace and just sees the
    // new one appear in the dropdown — silent and confusing (this is PB-820).
    onSuccess: async (_, invitationId) => {
      const invitation = myInvitations.find((i) => i.id === invitationId);
      queryClient.invalidateQueries({ queryKey: workspaceKeys.myInvitations() });
      // staleTime: 0 forces a real network fetch — we need the joined workspace
      // in the list before we can resolve its slug for navigation.
      const list = await queryClient.fetchQuery({
        ...workspaceListOptions(),
        staleTime: 0,
      });
      const joined = invitation
        ? list.find((w) => w.id === invitation.workspace_id)
        : null;
      if (joined) {
        push(paths.workspace(joined.slug).issues());
      }
    },
  });
  const declineInvitationMut = useMutation({
    mutationFn: (id: string) => api.declineInvitation(id),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: workspaceKeys.myInvitations() });
    },
  });

  const createIssueShortcut = useShortcut("createIssue");
  const archiveChatSession = useSetChatSessionArchived();

  const startChatFromSidebar = useCallback(
    (agent: Agent | null) => {
      setOpenMobile(false);
      useChatStore.getState().supersedeAgentIntent();
      push(agent ? p.chatWithAgent(agent.id) : p.chat());
    },
    [p, push, setOpenMobile],
  );
  const selectChatSessionFromSidebar = useCallback((session: ChatSession) => {
    setOpenMobile(false);
    const chatStore = useChatStore.getState();
    // This is the same intent-arbitration boundary used by ChatPage's compact
    // list. The URL push also removes a still-pending `?agent=` deep link
    // immediately; the revision protects the async gap before navigation
    // commits.
    chatStore.supersedeAgentIntent();
    chatStore.setSelectedAgentId(session.agent_id);
    chatStore.setActiveSession(session.id);
    push(`${chatHref}?session=${encodeURIComponent(session.id)}`);
  }, [chatHref, push, setOpenMobile]);
  const openChatTopics = useCallback(() => {
    setOpenMobile(false);
    const chatStore = useChatStore.getState();
    chatStore.supersedeAgentIntent();
    chatStore.requestTopicsView();
    chatStore.setActiveSession(null);
  }, [setOpenMobile]);
  const openChatProfile = useCallback(() => {
    setOpenMobile(false);
    push(activeChatAgent ? p.agentDetail(activeChatAgent.id) : p.agents());
  }, [activeChatAgent, p, push, setOpenMobile]);
  const openChatTasks = useCallback(() => {
    setOpenMobile(false);
    push(p.issues());
  }, [p, push, setOpenMobile]);

  if (isChatRoute) {
    return (
      <LobeAgentSidebar
        activeAgent={activeChatAgent}
        activeSessionId={activeChatSessionId}
        agents={chatAgents}
        availableAgents={availableChatAgents}
        chatHref={chatHref}
        chatUnreadCount={chatUnreadCount}
        headerClassName={headerClassName}
        headerStyle={headerStyle}
        acceptingInvitation={acceptInvitationMut.isPending}
        decliningInvitation={declineInvitationMut.isPending}
        myInvitations={myInvitations}
        onAcceptInvitation={(id) => acceptInvitationMut.mutate(id)}
        onArchive={(session) => {
          const chatStore = useChatStore.getState();
          if (session.id === activeChatSessionId) {
            chatStore.supersedeAgentIntent();
            if (isCompact) {
              chatStore.setActiveSession(null);
            } else {
              const history = sortChatSessions(
                chatSessions.filter((item) => item.status !== "archived"),
              );
              const index = history.findIndex((item) => item.id === session.id);
              const next = history[index + 1] ?? history[index - 1] ?? null;
              if (next) {
                chatStore.setSelectedAgentId(next.agent_id);
                chatStore.setActiveSession(next.id);
              } else {
                chatStore.setActiveSession(null);
              }
            }
          }
          archiveChatSession.mutate({ sessionId: session.id, archived: true });
        }}
        onDeclineInvitation={(id) => declineInvitationMut.mutate(id)}
        onOpenProfile={openChatProfile}
        onOpenSearch={() => {
          setOpenMobile(false);
          useSearchStore.getState().setOpen(true);
        }}
        onOpenTasks={openChatTasks}
        onOpenTopics={openChatTopics}
        onSelectSession={selectChatSessionFromSidebar}
        onStartChat={startChatFromSidebar}
        onSwitchWorkspace={(nextWorkspace) => {
          setOpenMobile(false);
          push(paths.workspace(nextWorkspace.slug).issues());
        }}
        onCreateWorkspace={() => {
          setOpenMobile(false);
          push(paths.newWorkspace());
        }}
        otherWorkspaceUnread={otherWorkspaceUnread}
        sidebarState={sidebarState}
        setHoverRevealSuspended={setHoverRevealSuspended}
        sessions={chatSessions}
        topSlot={topSlot}
        unreadWorkspaceIds={unreadWsIds}
        user={user}
        userId={userId}
        workspace={workspace}
        workspaces={workspaces}
        workspaceCreationDisabled={workspaceCreationDisabled}
        onLogout={logout}
      />
    );
  }

  return (
      <Sidebar variant="inset">
        {topSlot}
        {/* Workspace Switcher */}
        <SidebarHeader className={cn("py-3", headerClassName)} style={headerStyle}>
          <SidebarMenu>
            <SidebarMenuItem>
              <DropdownMenu onOpenChange={setHoverRevealSuspended}>
                <DropdownMenuTrigger
                  render={
                    <SidebarMenuButton>
                      <span className="relative">
                        <WorkspaceAvatar name={workspace?.name ?? "M"} avatarUrl={workspace?.avatar_url} size="sm" />
                        {/* Shared brand dot: a pending invitation OR another
                            workspace with unread inbox items. The active
                            workspace's own unread stays on the Inbox nav count
                            (below), so it is deliberately excluded here. */}
                        {(myInvitations.length > 0 || otherWorkspaceUnread) && (
                          <span className="absolute -top-0.5 -right-0.5 size-2 rounded-full bg-brand ring-1 ring-sidebar" />
                        )}
                      </span>
                      <span className="flex-1 truncate font-medium">
                        {workspace?.name ?? "Patchbay"}
                      </span>
                      <ChevronDown className="size-3 text-sidebar-icon-secondary" />
                    </SidebarMenuButton>
                  }
                />
                <DropdownMenuContent
                  className="w-auto min-w-56"
                  align="start"
                  side="bottom"
                  sideOffset={4}
                >
                  <DropdownMenuGroup>
                    <DropdownMenuLabel className="text-caption text-sidebar-text-secondary">
                      {t(($) => $.sidebar.workspaces_label)}
                    </DropdownMenuLabel>
                    {workspaces.map((ws) => (
                      <DropdownMenuItem
                        key={ws.id}
                        render={
                          <AppLink href={paths.workspace(ws.slug).issues()} />
                        }
                      >
                        <WorkspaceAvatar name={ws.name} avatarUrl={ws.avatar_url} size="sm" />
                        <span className="flex-1 truncate">{ws.name}</span>
                        {/* Points at the specific workspace holding unread
                            inbox items. Sits in the same right-edge slot as the
                            active-workspace check; the active workspace is
                            excluded (its unread is the Inbox nav count), so dot
                            and check never collide on one row. */}
                        {ws.id !== workspace?.id && unreadWsIds.has(ws.id) && (
                          <span className="size-2 rounded-full bg-brand" />
                        )}
                        {ws.id === workspace?.id && (
                          <Check className="h-3.5 w-3.5 text-primary" />
                        )}
                      </DropdownMenuItem>
                    ))}
                    {!workspaceCreationDisabled && (
                      <DropdownMenuItem
                        onClick={() => push(paths.newWorkspace())}
                      >
                        <Plus className="h-3.5 w-3.5" />
                        {t(($) => $.sidebar.create_workspace)}
                      </DropdownMenuItem>
                    )}
                  </DropdownMenuGroup>
                  {myInvitations.length > 0 && (
                    <>
                      <DropdownMenuSeparator />
                      <DropdownMenuGroup>
                        <DropdownMenuLabel className="text-caption text-sidebar-text-secondary">
                          {t(($) => $.sidebar.pending_invitations_label)}
                        </DropdownMenuLabel>
                        {myInvitations.map((inv) => (
                          <div key={inv.id} className="flex items-center gap-2 px-2 py-1.5">
                            <WorkspaceAvatar name={inv.workspace_name ?? "W"} size="sm" />
                            <span className="flex-1 truncate text-body">{inv.workspace_name ?? t(($) => $.sidebar.invitation_workspace_fallback)}</span>
                            <button
                              type="button"
                              className="text-caption px-2 py-0.5 rounded bg-primary text-primary-foreground hover:bg-primary/90 disabled:opacity-50"
                              disabled={acceptInvitationMut.isPending}
                              onClick={(e) => {
                                e.stopPropagation();
                                acceptInvitationMut.mutate(inv.id);
                              }}
                            >
                              {t(($) => $.sidebar.invitation_join)}
                            </button>
                            <button
                              type="button"
                              className="text-caption px-2 py-0.5 rounded bg-muted text-sidebar-text-secondary hover:bg-muted/80 disabled:opacity-50"
                              disabled={declineInvitationMut.isPending}
                              onClick={(e) => {
                                e.stopPropagation();
                                declineInvitationMut.mutate(inv.id);
                              }}
                            >
                              {t(($) => $.sidebar.invitation_decline)}
                            </button>
                          </div>
                        ))}
                      </DropdownMenuGroup>
                    </>
                  )}
                </DropdownMenuContent>
              </DropdownMenu>
            </SidebarMenuItem>
          </SidebarMenu>
          <SidebarMenu>
            {searchSlot && (
              <SidebarMenuItem>
                {searchSlot}
              </SidebarMenuItem>
            )}
            <SidebarMenuItem>
              <SidebarMenuButton
                className="text-sidebar-text-secondary"
                onClick={() => openCreateIssueWithPreference()}
              >
                <span className="relative">
                  <SquarePen className="text-sidebar-icon-secondary" />
                  <DraftDot />
                </span>
                <span>{t(($) => $.sidebar.new_issue)}</span>
                {createIssueShortcut ? (
                  <ShortcutKeycaps shortcut={createIssueShortcut} decorative className="pointer-events-none ml-auto" />
                ) : null}
              </SidebarMenuButton>
            </SidebarMenuItem>
          </SidebarMenu>
        </SidebarHeader>

        {/* Navigation */}
        <SidebarContent ref={sidebarScrollRef} style={sidebarFadeStyle}>
          <SidebarGroup>
            <SidebarGroupContent>
              <SidebarMenu className="gap-0.5">
                {personalNav.map((item) => {
                  const href = p[item.key]();
                  const Icon = routeIconForPath(href);
                  const isActive = isNavActive(pathname, href);
                  return (
                    <SidebarMenuItem key={item.key}>
                      <SidebarMenuButton
                        isActive={isActive}
                        render={<AppLink href={href} />}
                        className="text-sidebar-text-secondary hover:not-data-active:bg-sidebar-item-hover data-active:bg-sidebar-item-active data-active:text-sidebar-item-active-foreground"
                      >
                        <Icon className={sidebarNavIconClassName(isActive)} />
                        <span>{t(($) => $.nav[item.labelKey])}</span>
                        {item.key === "inbox" && unreadCount > 0 && (
                          <CappedNumberFlow
                            value={unreadCount}
                            animated={false}
                            className="ml-auto text-caption"
                          />
                        )}
                        {item.key === "chat" && chatUnreadCount > 0 && (
                          <CappedNumberFlow
                            value={chatUnreadCount}
                            animated={false}
                            className="ml-auto text-caption"
                          />
                        )}
                      </SidebarMenuButton>
                    </SidebarMenuItem>
                  );
                })}
              </SidebarMenu>
            </SidebarGroupContent>
          </SidebarGroup>

          {visiblePinned.length > 0 && (
            <Collapsible defaultOpen>
              <SidebarGroup className="group/pinned">
                <SidebarGroupLabel
                  render={<CollapsibleTrigger />}
                  className="group/trigger cursor-pointer hover:bg-sidebar-item-hover hover:text-sidebar-item-active-foreground"
                >
                  <span>{t(($) => $.sidebar.pinned_label)}</span>
                  <ChevronRight className="!size-3 ml-1 stroke-[2.5] text-sidebar-icon-secondary transition-transform duration-200 group-data-[panel-open]/trigger:rotate-90" />
                  <span className="ml-auto text-micro text-sidebar-text-secondary opacity-0 transition-opacity group-hover/pinned:opacity-100">{visiblePinned.length}</span>
                </SidebarGroupLabel>
                <CollapsibleContent>
                  <SidebarGroupContent>
                    <DndContext sensors={sensors} collisionDetection={closestCenter} onDragStart={handleDragStart} onDragEnd={handleDragEnd}>
                      <SortableContext items={visiblePinned.map((p) => p.id)} strategy={verticalListSortingStrategy}>
                        <SidebarMenu className="gap-0.5">
                          {visiblePinned.map((pin: PinnedItem) => (
                            <PinRow
                              key={pin.id}
                              pin={pin}
                              href={getPinHref(pin)}
                              pathname={pathname}
                              onUnpin={() => deletePin.mutate({ itemType: pin.item_type, itemId: pin.item_id })}
                              wsId={wsId ?? ""}
                            />
                          ))}
                        </SidebarMenu>
                      </SortableContext>
                    </DndContext>
                  </SidebarGroupContent>
                </CollapsibleContent>
              </SidebarGroup>
            </Collapsible>
          )}

          <SidebarGroup>
            <SidebarGroupLabel>{t(($) => $.sidebar.workspace_group)}</SidebarGroupLabel>
            <SidebarGroupContent>
              <SidebarMenu className="gap-0.5">
                {workspaceNav.map((item) => {
                  const href = p[item.key]();
                  const Icon = routeIconForPath(href);
                  const isActive = !isActivePinnedRoute && isNavActive(pathname, href);
                  return (
                    <SidebarMenuItem key={item.key}>
                      <SidebarMenuButton
                        isActive={isActive}
                        render={<AppLink href={href} />}
                        className="text-sidebar-text-secondary hover:not-data-active:bg-sidebar-item-hover data-active:bg-sidebar-item-active data-active:text-sidebar-item-active-foreground"
                      >
                        <Icon className={sidebarNavIconClassName(isActive)} />
                        <span>{t(($) => $.nav[item.labelKey])}</span>
                      </SidebarMenuButton>
                    </SidebarMenuItem>
                  );
                })}
              </SidebarMenu>
            </SidebarGroupContent>
          </SidebarGroup>

          <SidebarGroup>
            <SidebarGroupLabel>{t(($) => $.sidebar.configure_group)}</SidebarGroupLabel>
            <SidebarGroupContent>
              <SidebarMenu className="gap-0.5">
                {configureNav.map((item) => {
                  const href = p[item.key]();
                  const Icon = routeIconForPath(href);
                  const isActive = isNavActive(pathname, href);
                  return (
                    <SidebarMenuItem key={item.key}>
                      <SidebarMenuButton
                        isActive={isActive}
                        render={<AppLink href={href} />}
                        className="text-sidebar-text-secondary hover:not-data-active:bg-sidebar-item-hover data-active:bg-sidebar-item-active data-active:text-sidebar-item-active-foreground"
                      >
                        <Icon className={sidebarNavIconClassName(isActive)} />
                        <span>{t(($) => $.nav[item.labelKey])}</span>
                      </SidebarMenuButton>
                    </SidebarMenuItem>
                  );
                })}
              </SidebarMenu>
            </SidebarGroupContent>
          </SidebarGroup>
        </SidebarContent>

        <SidebarFooter className="p-1">
          <SidebarMenu>
            <SidebarMenuItem>
              <div className="flex min-w-0 items-center gap-1">
                <DropdownMenu onOpenChange={setHoverRevealSuspended}>
                  <DropdownMenuTrigger
                    render={
                      <SidebarMenuButton
                        size="lg"
                        className="min-w-0 flex-1 text-left"
                      >
                        <ActorAvatar
                          name={user?.name ?? ""}
                          initials={(user?.name ?? "U").charAt(0).toUpperCase()}
                          avatarUrl={resolvePublicFileUrl(user?.avatar_url)}
                          size="sm"
                        />
                        <span className="min-w-0 flex-1">
                          <span className="block truncate text-body font-medium leading-tight">
                            {user?.name}
                          </span>
                          <span className="block truncate text-caption text-sidebar-text-secondary leading-tight">
                            {user?.email}
                          </span>
                        </span>
                        <ChevronUp className="ml-auto size-4 text-sidebar-icon-secondary" />
                      </SidebarMenuButton>
                    }
                  />
                  <DropdownMenuContent
                    side="top"
                    align="start"
                    sideOffset={8}
                    className="min-w-56"
                  >
                    <div className="flex items-center gap-2.5 px-2 py-1.5">
                      <ActorAvatar
                        name={user?.name ?? ""}
                        initials={(user?.name ?? "U").charAt(0).toUpperCase()}
                        avatarUrl={resolvePublicFileUrl(user?.avatar_url)}
                        size="lg"
                      />
                      <div className="min-w-0 flex-1">
                        <p className="truncate text-body font-medium leading-tight">
                          {user?.name}
                        </p>
                        <p className="truncate text-caption text-sidebar-text-secondary leading-tight">
                          {user?.email}
                        </p>
                      </div>
                    </div>
                    <DropdownMenuSeparator />
                    <DropdownMenuGroup>
                      <DropdownMenuItem variant="destructive" onClick={logout}>
                        <LogOut className="h-3.5 w-3.5" />
                        {t(($) => $.sidebar.log_out)}
                      </DropdownMenuItem>
                    </DropdownMenuGroup>
                  </DropdownMenuContent>
                </DropdownMenu>
                {sidebarState !== "collapsed" && (
                  <HelpLauncher onOpenChange={setHoverRevealSuspended} />
                )}
              </div>
            </SidebarMenuItem>
          </SidebarMenu>
        </SidebarFooter>
        <SidebarRail />
      </Sidebar>
  );
}
