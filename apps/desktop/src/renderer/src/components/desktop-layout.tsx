import { useEffect, useRef, useSyncExternalStore } from "react";
import { useQuery } from "@tanstack/react-query";
import { cn } from "@orvilo/ui/lib/utils";
import {
  useNavigationInputBindings,
  useTabHistory,
} from "@/hooks/use-tab-history";
import {
  SidebarInset,
  SidebarProvider,
  SidebarTrigger,
  useSidebar,
} from "@orvilo/ui/components/ui/sidebar";
import { ModalRegistry } from "@orvilo/views/modals/registry";
import {
  AppSidebar,
  GlobalShortcuts,
  NavigationProgress,
  ShellBreadcrumb,
  ShellHeaderActionsSlot,
  ShellHeaderProvider,
} from "@orvilo/views/layout";
import { SearchCommand, SearchTrigger } from "@orvilo/views/search";
import { GlobalRightSidebar, GlobalRightSidebarToggle } from "@orvilo/views/chat";
import { AgentThreadPanelLayout } from "@orvilo/views/agent-thread";
import { WorkspaceSlugProvider, paths, useCurrentWorkspace } from "@orvilo/core/paths";
import { workspaceListOptions } from "@orvilo/core/workspace";
import {
  useNavigation,
  type LinkClickIntent,
} from "@orvilo/views/navigation";
import { getCurrentSlug, subscribeToCurrentSlug } from "@orvilo/core/platform";
import { useDesktopUnreadBadge } from "@orvilo/views/platform";
import {
  DesktopNavigationProvider,
  routeContentLinkPath,
} from "@/platform/navigation";
import { TabContent } from "./tab-content";
import { WindowOverlay } from "./window-overlay";
import { useWindowOverlayStore } from "@/stores/window-overlay-store";
import {
  TRAFFIC_LIGHT_CLUSTER_END,
  TRAFFIC_LIGHT_CONTENT_GAP,
  contentInsetAfterTrafficLights,
} from "../../../shared/window-chrome";

const TOP_BAR_HEIGHT_CLASS = "h-12";
const pinTriggerClassName =
  "flex size-7 items-center justify-center rounded-md bg-transparent text-faint-foreground shadow-none hover:bg-muted hover:text-foreground aria-expanded:bg-transparent! aria-expanded:text-faint-foreground!";
const noDragStyle = { WebkitAppRegion: "no-drag" } as React.CSSProperties;
const dragStyle = { WebkitAppRegion: "drag" } as React.CSSProperties;

function trafficLightEndPx(): number {
  return window.desktopAPI.appInfo?.os === "macos" ? TRAFFIC_LIGHT_CLUSTER_END : 0;
}

function SidebarPinTrigger() {
  const { toggleSidebar, open, guardCollapsedHoverReveal } = useSidebar();
  const toggledFromPointerRef = useRef(false);

  return (
    <SidebarTrigger
      aria-label="Toggle sidebar"
      title="Toggle sidebar"
      className={pinTriggerClassName}
      style={noDragStyle}
      onPointerDown={(event) => {
        if (event.button !== 0) return;
        toggledFromPointerRef.current = true;
        event.preventDefault();
        // The pin sits inside the sidebar. Collapsing without a rail-leave
        // guard would pop the floating column back under the cursor.
        if (open) guardCollapsedHoverReveal();
        toggleSidebar();
      }}
      onClick={(event) => {
        const pairedPointerClick = toggledFromPointerRef.current && event.detail > 0;
        toggledFromPointerRef.current = false;
        if (pairedPointerClick) event.preventDefault();
      }}
    />
  );
}

function pinOffsetPx(): number {
  return contentInsetAfterTrafficLights(trafficLightEndPx());
}

// Pin lives in the sidebar title row as a `no-drag` child of a drag parent —
// the Electron-documented pattern. A `position: fixed` overlay is not a
// descendant of any drag region, so Chromium's app-region hit test still
// treated those pixels as the native titlebar.
function SidebarTopSpacer() {
  return (
    <div
      data-slot="window-toolbar"
      className={cn("flex shrink-0 items-center overflow-visible", TOP_BAR_HEIGHT_CLASS)}
      style={dragStyle}
    >
      <div className="h-full shrink-0" style={{ width: pinOffsetPx() }} />
      <SidebarPinTrigger />
      <div className="min-w-0 flex-1" />
    </div>
  );
}

function useNativeNavigationGestures() {
  const { goBack, goForward } = useTabHistory();

  useEffect(() => {
    return window.desktopAPI.onNavigationGesture((gesture) => {
      if (gesture === "back") {
        goBack();
      } else {
        goForward();
      }
    });
  }, [goBack, goForward]);
}


// Collapsed, the pin overflows the icon rail into this header. Keep a
// `pointer-events-none` hole the same size as the pin (`w-7` / `size-7`)
// so the inset cannot eat the overflow clicks.
function MainTopBar({ sidebarAvailable }: { sidebarAvailable: boolean }) {
  const { open, isCompact } = useSidebar();
  const sidebarOutOfFlow = !open || isCompact;

  return (
    <header
      className={cn(
        "relative flex shrink-0 items-center gap-2 border-b border-border/60 pr-3",
        TOP_BAR_HEIGHT_CLASS,
      )}
    >
      {!sidebarAvailable ? (
        <div className="flex h-full shrink-0 items-center" style={dragStyle}>
          <div className="h-full shrink-0" style={{ width: pinOffsetPx() }} />
          <SidebarPinTrigger />
        </div>
      ) : sidebarOutOfFlow ? (
        <div aria-hidden className="pointer-events-none w-7 shrink-0" />
      ) : (
        <div aria-hidden className="w-3 shrink-0" style={dragStyle} />
      )}
      <div className="h-full min-w-0 flex-1" style={dragStyle}>
        <div className="flex h-full w-fit max-w-full items-center" style={noDragStyle}>
          <ShellBreadcrumb />
        </div>
      </div>
      <ShellHeaderActionsSlot style={noDragStyle} />
      <div style={noDragStyle}><GlobalRightSidebarToggle /></div>
    </header>
  );
}

// Keep the canvas on the same in-flow signal as the sidebar gap and top bar.
// Temporary hover reveal is an overlay and must not pull either sibling left.
function MainCanvas({ children }: { children: React.ReactNode }) {
  return (
    <div className="relative flex min-h-0 flex-1 flex-col overflow-hidden">
      {children}
    </div>
  );
}

function useInternalLinkHandler() {
  useEffect(() => {
    const handler = (e: Event) => {
      const detail = (
        e as CustomEvent<{ path?: string; disposition?: LinkClickIntent }>
      ).detail;
      if (!detail?.path) return;
      routeContentLinkPath(detail.path, detail.disposition);
    };
    window.addEventListener("orvilo:navigate", handler);
    return () => window.removeEventListener("orvilo:navigate", handler);
  }, []);
}

/**
 * Bridge between the renderer and the Electron main process for inbox-level
 * OS integration. Mounted inside WorkspaceSlugProvider so it can resolve the
 * current workspace's id for the badge hook.
 *
 * Two responsibilities:
 *   1. Mirror the unread inbox count onto the dock/taskbar badge.
 *   2. When the user clicks an OS notification, open the notified
 *      workspace's inbox focused on that item. The route uses the `slug`
 *      that the notification was *emitted* with — not the currently active
 *      workspace — so a notification from workspace A always opens A's
 *      inbox even if the user has since switched to workspace B. Marking
 *      the row read is handled by InboxPage's selected-item effect, which
 *      covers both click-to-select and URL-param-select paths.
 *
 * The click routes through `useNavigation().push` — NOT the
 * `orvilo:navigate` event, whose handler `openTab`s into the ACTIVE
 * workspace's tab group. The navigation adapter detects a cross-workspace
 * path and translates it into `switchWorkspace(slug, path)`, so clicking a
 * workspace-A notification while B is active performs a real workspace
 * switch instead of mounting A's inbox inside B's tab group (#3766).
 */
function DesktopInboxBridge() {
  const workspace = useCurrentWorkspace();
  useDesktopUnreadBadge(workspace?.id ?? null);
  const { push } = useNavigation();
  // The adapter identity changes with the active tab's location; the ref
  // keeps the main-process subscription stable across navigations.
  const pushRef = useRef(push);
  useEffect(() => {
    pushRef.current = push;
  }, [push]);

  useEffect(() => {
    return window.desktopAPI.onInboxOpen(({ slug, issueKey }) => {
      if (!slug) return;
      const inboxPath = `${paths.workspace(slug).inbox()}?issue=${encodeURIComponent(issueKey)}`;
      pushRef.current(inboxPath);
    });
  }, []);

  return null;
}

export function DesktopShell() {
  const settingsOpen = useWindowOverlayStore((s) => s.overlay?.type === "settings");
  useInternalLinkHandler();
  useNativeNavigationGestures();
  useNavigationInputBindings();

  // Reactive read of current workspace slug from the platform singleton.
  // On first mount, it is null until WorkspaceRouteLayout (inside the tab
  // router) sets it. Once set, the sidebar and other shell-level components
  // can resolve workspace-scoped paths via useWorkspacePaths().
  const currentSlug = useSyncExternalStore(
    subscribeToCurrentSlug,
    getCurrentSlug,
    () => null,
  );
  // Chrome gates on "the slug still resolves to a workspace", NOT on "the
  // singleton is non-null" (MUL-6231 / #7021). The singleton is mutable
  // process state that no single owner keeps in lockstep with the workspace
  // list, so after the active workspace is deleted it can still hold the dead
  // slug for a beat. Everything below mounts workspace-scoped components —
  // SearchCommand calls useWorkspaceId(), which THROWS when the workspace is
  // gone from the list. Nothing above this in the desktop tree is an error
  // boundary, so that throw used to unmount the whole renderer and leave a
  // blank, unresponsive window.
  //
  // Deriving from the list cache makes this the same gate web uses
  // (DashboardGuard's `!workspace` check in packages/views/layout), so both
  // shells drop workspace-scoped chrome on exactly the same signal instead of
  // diverging. TabContent stays outside the gate: it must always render so
  // the tab router can mount WorkspaceRouteLayout, which is what populates
  // the singleton in the first place.
  const { data: workspaces = [] } = useQuery(workspaceListOptions());
  const slug =
    currentSlug && workspaces.some((w) => w.slug === currentSlug)
      ? currentSlug
      : null;
  return (
    <DesktopNavigationProvider>
      {/* WorkspaceSlugProvider accepts null — components that need slug
          use useWorkspaceSlug() (nullable) or useRequiredWorkspaceSlug()
          (throws). TabContent MUST always render so the tab router can
          mount WorkspaceRouteLayout, which calls setCurrentWorkspace()
          to populate the slug. The sidebar gates on the resolved slug
          (see above) to avoid the useRequiredWorkspaceSlug and
          useWorkspaceId throws. Zero-workspace users see the
          window-level overlay (new-workspace flow) triggered by
          IndexRedirect, not a route. */}
      <WorkspaceSlugProvider slug={slug}>
        <DesktopInboxBridge />
        <div
          data-slot="desktop-dashboard"
          inert={settingsOpen}
          className={cn("flex h-screen", "bg-app-shell")}
        >
          {/* The sidebar title row parks the one persistent trigger beside the
              traffic lights. Keep the provider flag so page headers do not
              add a second fallback trigger inside the canvas. */}
          <SidebarProvider
            hasExternalTrigger
            hoverReveal
            compactBehavior="collapse"
            autoCollapse={false}
            style={
              {
                "--desktop-traffic-light-end": `${trafficLightEndPx()}px`,
                "--desktop-content-gutter": `${TRAFFIC_LIGHT_CONTENT_GAP}px`,
              } as React.CSSProperties
            }
            className={cn(
              "flex-1 [--sidebar-width:260px] [--sidebar-border:transparent]",
              "bg-app-shell",
            )}
          >
            {slug && <GlobalShortcuts />}
            <ShellHeaderProvider>
              {slug && (
                <AppSidebar
                  topSlot={<SidebarTopSpacer />}
                  searchSlot={<SearchTrigger />}
                />
              )}
              <SidebarInset className="min-w-0 flex-row! rounded-none! ring-0! shadow-none overflow-hidden">
                <div className="flex min-h-0 min-w-0 flex-1 flex-col overflow-hidden" data-slot="shell-main-column">
                <MainTopBar sidebarAvailable={!!slug} />
                <MainCanvas>
                  {/* Same indicator, same anchor as web: DashboardLayout puts it
                      at the top of SidebarInset, and MainCanvas is desktop's
                      equivalent relative/overflow-hidden content box. Desktop
                      used to have no navigation feedback at all — a click just
                      froze until the destination committed (MUL-6404). */}
                  <AgentThreadPanelLayout>
                    <NavigationProgress />
                    <TabContent />
                  </AgentThreadPanelLayout>
                </MainCanvas>
                </div>
                {slug && <GlobalRightSidebar />}
              </SidebarInset>
            </ShellHeaderProvider>
          </SidebarProvider>
        </div>
        {slug && <ModalRegistry />}
        {slug && <SearchCommand />}
        <WindowOverlay />
      </WorkspaceSlugProvider>
    </DesktopNavigationProvider>
  );
}
