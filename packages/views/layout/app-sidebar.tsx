"use client";

import { issueStatusCategory } from "@orvilo/core/issues";
import React, { useCallback, useEffect, useRef, useState } from "react";
import { cn } from "@orvilo/ui/lib/utils";
import { useScrollFade } from "@orvilo/ui/hooks/use-scroll-fade";
import { AppLink, useNavigation } from "../navigation";
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
  ChevronRight,
  SquarePen,
  X,
} from "lucide-react";
import { NavWorkspace } from "@orvilo/ui/components/blocks/app-shell-9/components/nav-workspace";
import { Tooltip, TooltipTrigger, TooltipContent } from "@orvilo/ui/components/ui/tooltip";
import { Collapsible, CollapsibleTrigger, CollapsibleContent } from "@orvilo/ui/components/ui/collapsible";
import { CappedNumberFlow } from "@orvilo/ui/components/ui/number-flow";
import { StatusIcon } from "../issues/components/status-icon";
import { useIssueDraftStore } from "@orvilo/core/issues/stores/draft-store";
import { openCreateIssueWithPreference } from "@orvilo/core/issues/stores/create-mode-store";
import {
  Sidebar,
  SidebarContent,
  SidebarFooter,
  SidebarGroup,
  SidebarGroupContent,
  SidebarGroupLabel,
  SidebarMenu,
  SidebarMenuBadge,
  SidebarMenuButton,
  SidebarMenuItem,
  SidebarRail,
  useSidebar,
} from "@orvilo/ui/components/ui/sidebar";
import {
  DropdownMenuLabel,
  DropdownMenuSeparator,
} from "@orvilo/ui/components/ui/dropdown-menu";
import { useAuthStore } from "@orvilo/core/auth";
import { issueViewDetailOptions } from "@orvilo/core/issue-views/queries";
import {
  issueViewContainerKey,
  useActiveIssueViewStore,
} from "@orvilo/core/issue-views/active-view-store";
import { useCurrentWorkspace, useWorkspacePaths, paths } from "@orvilo/core/paths";
import { workspaceListOptions, myInvitationListOptions, workspaceKeys } from "@orvilo/core/workspace/queries";
import { resolvePublicFileUrl } from "@orvilo/core/workspace/avatar-url";
import { useQuery, useMutation, useQueryClient } from "@tanstack/react-query";
import { inboxKeys, deduplicateInboxItems, inboxUnreadSummaryOptions, hasOtherWorkspaceUnread, unreadWorkspaceIds } from "@orvilo/core/inbox/queries";
import { chatSessionsOptions } from "@orvilo/core/chat/queries";
import { countUnreadChatMessages } from "@orvilo/core/chat/unread";
import { useChatStore } from "@orvilo/core/chat";
import { api, ApiError } from "@orvilo/core/api";
import { useConfigStore, useFeatureEnabled } from "@orvilo/core/config";
import { BILLING_WORKSPACE_SUBSCRIPTIONS_FLAG } from "@orvilo/core/feature-flags";
import { pinListOptions } from "@orvilo/core/pins/queries";
import { useDeletePin, useReorderPins } from "@orvilo/core/pins/mutations";
import { issueDetailOptions } from "@orvilo/core/issues/queries";
import { projectDetailOptions } from "@orvilo/core/projects/queries";
import type { PinnedItem } from "@orvilo/core/types";
import { useLogout } from "../auth";
import { ProjectIcon } from "../projects/components/project-icon";
import { routeIconForPath } from "./route-icon-components";
import { useT } from "../i18n";
import { useShortcut } from "@orvilo/core/shortcuts";
import { ShortcutKeycaps } from "../common/shortcut-keycaps";
import { useAppForeground } from "../common/use-app-foreground";

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

// Nav items reference WorkspacePaths method names so they can be resolved
// against the current workspace slug at render time (see AppSidebar body).
// Only parameterless paths are valid nav destinations.
type NavKey =
  | "inbox"
  | "chat"
  | "myIssues"
  | "issues"
  | "taskGraph"
  | "projects"
  | "automations"
  | "agents"
  | "teams"
  | "channels"
  | "usage"
  | "runtimes"
  | "skills"
  | "settings";

// Static schema (key only) — labels resolved at render via useT("layout"),
// icons derived from the destination path via routeIconForPath.
type NavLabelKey =
  | "inbox"
  | "chat"
  | "my_issues"
  | "issues"
  | "task_graph"
  | "projects"
  | "automations"
  | "agents"
  | "teams"
  | "channels"
  | "usage"
  | "runtimes"
  | "skills"
  | "settings";

// Nav icons are NOT declared here: they are derived from each item's
// destination path at render time, so the sidebar and the desktop tab bar
// always agree. See route-icon-components.tsx.
const personalNav: { key: NavKey; labelKey: NavLabelKey }[] = [
  { key: "inbox", labelKey: "inbox" },
  { key: "chat", labelKey: "chat" },
  { key: "myIssues", labelKey: "my_issues" },
];

const workspaceNav: { key: NavKey; labelKey: NavLabelKey }[] = [
  { key: "issues", labelKey: "issues" },
  { key: "projects", labelKey: "projects" },
  { key: "automations", labelKey: "automations" },
  { key: "agents", labelKey: "agents" },
  { key: "teams", labelKey: "teams" },
  { key: "channels", labelKey: "channels" },
  { key: "usage", labelKey: "usage" },
];

const configureNav: { key: NavKey; labelKey: NavLabelKey }[] = [
  { key: "runtimes", labelKey: "runtimes" },
  { key: "skills", labelKey: "skills" },
  { key: "settings", labelKey: "settings" },
];

function DraftDot() {
  const hasDraft = useIssueDraftStore((state) => state.hasDraft());
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
          isDragging && "pointer-events-none",
        )}
      >
        {iconNode}
        <span
          className="min-w-0 flex-1 truncate whitespace-nowrap transition-[max-width,opacity] duration-200 group-data-[collapsible=icon]:max-w-0 group-data-[collapsible=icon]:opacity-0"
          style={{
            maskImage: "linear-gradient(to right, black calc(100% - 12px), transparent)",
            WebkitMaskImage: "linear-gradient(to right, black calc(100% - 12px), transparent)",
          }}
        >{label}</span>
        <Tooltip>
          <TooltipTrigger
            render={<span role="button" tabIndex={0} aria-label={t(($) => $.sidebar.unpin_tooltip)} />}
            className="hidden size-2.5 shrink-0 items-center justify-center rounded-sm text-accent-foreground/80 group-hover/pin:flex hover:text-foreground"
            onClick={(event) => {
              event.preventDefault();
              event.stopPropagation();
              onUnpin();
            }}
            onKeyDown={(event) => {
              if (event.key !== "Enter" && event.key !== " ") return;
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
        iconNode={<Layers className="shrink-0" />}
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
      <StatusIcon
        status={issue.status}
        category={issueStatusCategory(issue) ?? undefined}
        className="shrink-0"
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
      <div className="flex h-7 w-full items-center gap-2 px-2">
        <div className="size-3.5 shrink-0 rounded-sm bg-muted/40" />
        <div className="h-3 w-24 rounded bg-muted/40" />
      </div>
    </SidebarMenuItem>
  );
}

type ReuiRouteItem = {
  label: string;
  href: string;
  isActive: boolean;
  Icon: React.ComponentType<{ className?: string }>;
  badge?: React.ReactNode;
};

function ReuiNavGroup({
  label,
  items,
}: {
  label?: string;
  items: ReuiRouteItem[];
}) {
  return (
    <SidebarGroup>
      {label ? (
        <SidebarGroupLabel className="in-data-[state=collapsed]:hidden">
          {label}
        </SidebarGroupLabel>
      ) : null}
      <SidebarGroupContent>
        <SidebarMenu>
          {items.map(({ label: itemLabel, href, isActive, Icon, badge }) => (
            <SidebarMenuItem key={href}>
              <SidebarMenuButton
                tooltip={itemLabel}
                isActive={isActive}
                render={<AppLink href={href} />}
              >
                <Icon aria-hidden="true" />
                <span className="min-w-0 flex-1 truncate transition-[max-width,opacity] duration-200 group-data-[collapsible=icon]:max-w-0 group-data-[collapsible=icon]:opacity-0">
                  {itemLabel}
                </span>
                {badge !== undefined ? (
                  <SidebarMenuBadge>{badge}</SidebarMenuBadge>
                ) : null}
              </SidebarMenuButton>
            </SidebarMenuItem>
          ))}
        </SidebarMenu>
      </SidebarGroupContent>
    </SidebarGroup>
  );
}

interface AppSidebarProps {
  /** Rendered above the sidebar content (e.g. desktop traffic light spacer) */
  topSlot?: React.ReactNode;
  /** Rendered above the new-issue button (e.g. the search trigger) */
  searchSlot?: React.ReactNode;
}

export function AppSidebar({
  topSlot,
  searchSlot,
}: AppSidebarProps = {}) {
  const { t } = useT("layout");
  const { pathname, searchParams, push, openInNewTab } = useNavigation();
  const search = searchParams?.toString() ?? "";
  const user = useAuthStore((s) => s.user);
  const userId = useAuthStore((s) => s.user?.id);
  const logout = useLogout();
  const workspace = useCurrentWorkspace();
  const p = useWorkspacePaths();
  const { data: workspaces = EMPTY_WORKSPACES } = useQuery(workspaceListOptions());
  const { data: myInvitations = EMPTY_INVITATIONS } = useQuery(myInvitationListOptions());
  const workspaceCreationDisabled = useConfigStore((s) => s.workspaceCreationDisabled);
  const billingEnabled = useFeatureEnabled(
    BILLING_WORKSPACE_SUBSCRIPTIONS_FLAG,
    false,
  );

  // On a phone the sidebar is a Sheet covering the page, so navigating out of
  // it has to dismiss it — otherwise the destination renders underneath and the
  // tap reads as "nothing happened". Closing on `pathname` rather than on each
  // link's onClick covers every route out of here at once: the nav groups, the
  // pinned items, the workspace switcher's programmatic push, and anything
  // added later. `setOpenMobile` is a no-op on desktop, where the sheet is not
  // the sidebar's rendering at all.
  const { open: sidebarOpen, setOpenMobile, setHoverRevealSuspended } = useSidebar();
  useEffect(() => {
    setOpenMobile(false);
  }, [pathname, search, setOpenMobile]);

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
  // auto mark-read is suppressed (MUL-4485) so the reply stays unread — the
  // badge must count it, or the notification is silently eaten while the user
  // is away. A remembered selection while both surfaces are closed also still
  // counts, for the same reason.
  const activeChatSessionId = useChatStore((s) => s.activeSessionId);
  const floatingChatOpen = useChatStore((s) => s.isOpen);
  const appForeground = useAppForeground();
  const chatHref = p.chat();
  const viewedChatSessionId =
    appForeground && (floatingChatOpen || isNavActive(pathname, chatHref))
      ? activeChatSessionId
      : null;
  const chatUnreadCount = React.useMemo(
    () => countUnreadChatMessages(chatSessions, viewedChatSessionId),
    [chatSessions, viewedChatSessionId],
  );
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
    // new one appear in the dropdown — silent and confusing (this is MUL-820).
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

  const personalItems: ReuiRouteItem[] = personalNav.map((item) => {
    const href = p[item.key]();
    const Icon = routeIconForPath(href);
    const isActive = isNavActive(pathname, href);
    const badge =
      item.key === "inbox" && unreadCount > 0 ? (
        <CappedNumberFlow value={unreadCount} animated={false} />
      ) : item.key === "chat" && chatUnreadCount > 0 ? (
        <CappedNumberFlow value={chatUnreadCount} animated={false} />
      ) : undefined;
    return { label: t(($) => $.nav[item.labelKey]), href, isActive, Icon, badge };
  });
  const workspaceItems: ReuiRouteItem[] = workspaceNav.map((item) => {
    const href = p[item.key]();
    const Icon = routeIconForPath(href);
    const isActive =
      !isActivePinnedRoute &&
      (isNavActive(pathname, href) ||
        (item.key === "issues" && isNavActive(pathname, p.taskGraph())));
    return { label: t(($) => $.nav[item.labelKey]), href, isActive, Icon };
  });
  const configureItems: ReuiRouteItem[] = configureNav.map((item) => {
    const href = p[item.key]();
    const Icon = routeIconForPath(href);
    const isActive = isNavActive(pathname, href);
    return { label: t(($) => $.nav[item.labelKey]), href, isActive, Icon };
  });

  const userName = user?.name?.trim() || "User";
  const userInitials = userName
    .split(/\s+/)
    .slice(0, 2)
    .map((part) => part.charAt(0))
    .join("")
    .toUpperCase() || "U";
  const workspaceEntries = workspaces.map((ws) => ({
    id: ws.id,
    name: ws.name,
    hasUnread: ws.id !== workspace?.id && unreadWsIds.has(ws.id),
  }));
  const activeWorkspaceEntry = workspaceEntries.find((entry) => entry.id === workspace?.id);
  const createIssueShortcut = useShortcut("createIssue");
  const pendingInvitationMenu = myInvitations.length > 0 ? (
    <>
      <DropdownMenuSeparator />
      <DropdownMenuLabel>{t(($) => $.sidebar.pending_invitations_label)}</DropdownMenuLabel>
      {myInvitations.map((invitation) => {
        const invitationName = invitation.workspace_name ?? t(($) => $.sidebar.invitation_workspace_fallback);
        return (
          <div key={invitation.id} className="flex items-center gap-2 px-2 py-1.5">
            <span className="min-w-0 flex-1 truncate text-body">{invitationName}</span>
            <button
              type="button"
              className="rounded bg-primary px-2 py-0.5 text-caption text-primary-foreground hover:bg-primary/90 disabled:opacity-50"
              disabled={acceptInvitationMut.isPending}
              onClick={(event) => {
                event.stopPropagation();
                acceptInvitationMut.mutate(invitation.id);
              }}
            >
              {t(($) => $.sidebar.invitation_join)}
            </button>
            <button
              type="button"
              className="rounded bg-muted px-2 py-0.5 text-caption text-muted-foreground hover:bg-muted/80 disabled:opacity-50"
              disabled={declineInvitationMut.isPending}
              onClick={(event) => {
                event.stopPropagation();
                declineInvitationMut.mutate(invitation.id);
              }}
            >
              {t(($) => $.sidebar.invitation_decline)}
            </button>
          </div>
        );
      })}
    </>
  ) : null;

  return (
    <Sidebar collapsible="offcanvas" variant="inset">
      {sidebarOpen && topSlot}

      <SidebarContent ref={sidebarScrollRef} style={sidebarFadeStyle}>
        <div className="p-2 in-data-[state=collapsed]:flex in-data-[state=collapsed]:justify-center in-data-[state=collapsed]:px-1">
          {searchSlot}
        </div>

        <SidebarGroup className="py-0">
          <SidebarGroupContent>
            <SidebarMenu>
              <SidebarMenuItem>
                <SidebarMenuButton
                  tooltip={t(($) => $.sidebar.new_issue)}
                  aria-label={t(($) => $.sidebar.new_issue)}
                  onClick={() => openCreateIssueWithPreference()}
                >
                  <span className="relative">
                    <SquarePen aria-hidden="true" />
                    <DraftDot />
                  </span>
                  <span className="min-w-0 flex-1 truncate transition-[max-width,opacity] duration-200 group-data-[collapsible=icon]:max-w-0 group-data-[collapsible=icon]:opacity-0">
                    {t(($) => $.sidebar.new_issue)}
                  </span>
                  {createIssueShortcut ? (
                    <ShortcutKeycaps
                      shortcut={createIssueShortcut}
                      decorative
                      className="pointer-events-none ml-auto in-data-[state=collapsed]:hidden"
                    />
                  ) : null}
                </SidebarMenuButton>
              </SidebarMenuItem>
            </SidebarMenu>
          </SidebarGroupContent>
        </SidebarGroup>

        <ReuiNavGroup items={personalItems} />

        {visiblePinned.length > 0 ? (
          <Collapsible defaultOpen>
            <SidebarGroup className="group/pinned">
              <SidebarGroupLabel
                render={<CollapsibleTrigger />}
                className="in-data-[state=collapsed]:hidden"
              >
                <span>{t(($) => $.sidebar.pinned_label)}</span>
                <ChevronRight className="ml-1 size-3 opacity-60 transition-transform duration-200 group-data-[panel-open]/trigger:rotate-90" />
                <span className="ml-auto text-caption opacity-60">{visiblePinned.length}</span>
              </SidebarGroupLabel>
              <CollapsibleContent>
                <SidebarGroupContent>
                  <DndContext
                    sensors={sensors}
                    collisionDetection={closestCenter}
                    onDragStart={handleDragStart}
                    onDragEnd={handleDragEnd}
                  >
                    <SortableContext
                      items={visiblePinned.map((pin) => pin.id)}
                      strategy={verticalListSortingStrategy}
                    >
                      <SidebarMenu>
                        {visiblePinned.map((pin) => (
                          <PinRow
                            key={pin.id}
                            pin={pin}
                            href={getPinHref(pin)}
                            pathname={pathname}
                            onUnpin={() =>
                              deletePin.mutate({
                                itemType: pin.item_type,
                                itemId: pin.item_id,
                              })
                            }
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
        ) : null}

        <ReuiNavGroup
          label={t(($) => $.sidebar.workspace_group)}
          items={workspaceItems}
        />

        <div className="mt-auto">
          <ReuiNavGroup
            label={t(($) => $.sidebar.configure_group)}
            items={configureItems}
          />
        </div>
      </SidebarContent>

      <SidebarFooter className="pb-2 in-data-[state=collapsed]:px-1">
        <NavWorkspace
          showWorkspaceIcons={false}
          user={{
            name: userName,
            initials: userInitials,
            email: user?.is_guest === true ? t(($) => $.sidebar.guest_account) : user?.email,
            avatarUrl: resolvePublicFileUrl(user?.avatar_url),
          }}
          workspaces={workspaceEntries}
          activeWorkspace={
            activeWorkspaceEntry
              ? {
                  ...activeWorkspaceEntry,
                  hasUnread: myInvitations.length > 0 || otherWorkspaceUnread,
                }
              : undefined
          }
          labels={{
            menuAriaLabel: t(($) => $.sidebar.workspaces_label),
            workspaces: t(($) => $.sidebar.workspaces_label),
            createWorkspace: t(($) => $.sidebar.create_workspace),
            account: t(($) => $.sidebar.account),
            profile: t(($) => $.sidebar.profile),
            billing: t(($) => $.sidebar.billing),
            preferences: t(($) => $.sidebar.preferences),
            theme: t(($) => $.sidebar.theme),
            signOut: t(($) => $.sidebar.log_out),
            light: t(($) => $.sidebar.light),
            dark: t(($) => $.sidebar.dark),
            system: t(($) => $.sidebar.system),
          }}
          workspaceMenuContent={pendingInvitationMenu}
          onOpenChange={setHoverRevealSuspended}
          onSelectWorkspace={(workspaceId, event) => {
            const selected = workspaces.find((candidate) => candidate.id === workspaceId);
            if (!selected) return;
            const href = paths.workspace(selected.slug).issues();
            const wantsNewTab =
              event.button === 1 || event.metaKey || event.ctrlKey;
            if (wantsNewTab) {
              event.preventDefault();
              if (openInNewTab) {
                openInNewTab(href, selected.name, {
                  activate: event.button !== 1 && event.shiftKey,
                });
              } else {
                window.open(href, "_blank", "noopener,noreferrer");
              }
              return;
            }
            push(href);
          }}
          onCreateWorkspace={workspaceCreationDisabled ? undefined : () => push(paths.newWorkspace())}
          onProfile={() => push(`${p.settings()}?tab=profile`)}
          onBilling={
            billingEnabled ? () => push(`${p.settings()}?tab=billing`) : undefined
          }
          onPreferences={() => push(`${p.settings()}?tab=preferences`)}
          onSignOut={logout}
        />
      </SidebarFooter>

      {sidebarOpen && <SidebarRail />}
    </Sidebar>
  );

}
