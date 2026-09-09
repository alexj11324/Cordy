import { useEffect, useRef, useSyncExternalStore } from "react";
import { ChevronLeft, ChevronRight } from "lucide-react";
import { motion } from "motion/react";
import { useQuery } from "@tanstack/react-query";
import { cn } from "@orvilo/ui/lib/utils";
import {
  useNavigationInputBindings,
  useTabHistory,
} from "@/hooks/use-tab-history";
import {
  SidebarProvider,
  useSidebar,
} from "@orvilo/ui/components/ui/sidebar";
import { ModalRegistry } from "@orvilo/views/modals/registry";
import {
  AppSidebar,
  GlobalShortcuts,
  NavigationProgress,
} from "@orvilo/views/layout";
import { SearchCommand, SearchTrigger } from "@orvilo/views/search";
import { FloatingChat } from "@orvilo/views/chat";
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
import { TabBar } from "./tab-bar";
import { TabContent } from "./tab-content";
import { WindowOverlay } from "./window-overlay";
import { useWindowOverlayStore } from "@/stores/window-overlay-store";

const TOP_BAR_HEIGHT_CLASS = "h-12";
const WINDOW_TOOLBAR_CLEARANCE = 184;
const toolbarMotion = {
  type: "spring",
  stiffness: 420,
  damping: 38,
  mass: 0.8,
} as const;

function WindowToolbar() {
  const { canGoBack, canGoForward, goBack, goForward } = useTabHistory();
  const navButtonClassName =
    "flex size-7 items-center justify-center rounded-md text-faint-foreground transition-colors hover:bg-sidebar-accent hover:text-sidebar-accent-foreground disabled:pointer-events-none disabled:opacity-30";

  return (
    <div
      className={cn(
        "fixed left-0 top-0 z-30 flex w-[184px] shrink-0 items-center px-3",
        TOP_BAR_HEIGHT_CLASS,
      )}
      style={{ WebkitAppRegion: "drag" } as React.CSSProperties}
    >
      <div
        className="flex items-center gap-1 pl-[70px]"
        style={{ WebkitAppRegion: "no-drag" } as React.CSSProperties}
      >
        <div className="flex items-center gap-1">
          <button
            type="button"
            onClick={goBack}
            disabled={!canGoBack}
            aria-label="Go back"
            title="Go back"
            className={navButtonClassName}
            style={{ WebkitAppRegion: "no-drag" } as React.CSSProperties}
          >
            <ChevronLeft className="size-4" />
          </button>
          <button
            type="button"
            onClick={goForward}
            disabled={!canGoForward}
            aria-label="Go forward"
            title="Go forward"
            className={navButtonClassName}
            style={{ WebkitAppRegion: "no-drag" } as React.CSSProperties}
          >
            <ChevronRight className="size-4" />
          </button>
        </div>
      </div>
    </div>
  );
}

function SidebarTopSpacer() {
  return <div className={cn("shrink-0", TOP_BAR_HEIGHT_CLASS)} />;
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


// The main area's top bar doubles as a window drag region. Track `open`, which
// owns the sidebar's in-flow gap, rather than `state`, which also becomes
// expanded during a temporary hover overlay. A hover-revealed sidebar remains
// out of flow, so the tab strip must keep clearing the fixed window toolbar and
// native traffic lights.
function MainTopBar() {
  const { open, isCompact } = useSidebar();
  const sidebarOutOfFlow = !open || isCompact;

  return (
    <motion.header
      animate={{ paddingLeft: sidebarOutOfFlow ? WINDOW_TOOLBAR_CLEARANCE : 0 }}
      className={cn("relative shrink-0 flex items-center gap-2", TOP_BAR_HEIGHT_CLASS)}
      initial={false}
      transition={toolbarMotion}
    >
      <motion.div
        aria-hidden
        animate={{ left: sidebarOutOfFlow ? WINDOW_TOOLBAR_CLEARANCE : 0 }}
        className="absolute inset-y-0 right-0"
        initial={false}
        transition={toolbarMotion}
        style={{ WebkitAppRegion: "drag" } as React.CSSProperties}
      />
      <div className="relative z-10 flex h-full min-w-0 max-w-full items-center">
        <TabBar />
      </div>
    </motion.header>
  );
}

// Keep the canvas on the same in-flow signal as the sidebar gap and top bar.
// Temporary hover reveal is an overlay and must not pull either sibling left.
function MainCanvas({ children }: { children: React.ReactNode }) {
  const { open, isCompact } = useSidebar();
  const sidebarOutOfFlow = !open || isCompact;

  return (
    <motion.div
      animate={{ marginLeft: sidebarOutOfFlow ? 8 : 0 }}
      className="relative flex flex-1 min-h-0 flex-col overflow-hidden mr-2 mb-2 rounded-xl bg-page-canvas"
      initial={false}
      transition={toolbarMotion}
    >
      {children}
    </motion.div>
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
  const usesNativeVibrancy =
    window.desktopAPI.host === "electron" &&
    window.desktopAPI.appInfo?.os === "macos";

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
          className={cn(
            "flex h-screen",
            settingsOpen && "invisible",
            usesNativeVibrancy ? "bg-transparent" : "bg-app-shell",
          )}
        >
          {/* Non-macOS keeps the opaque app-shell wrapper. On macOS, the shell
              is transparent so Electron's native sidebar material can show
              through; descendants that need an opaque fill still read the
              app-shell token from --sidebar-wrapper-fill. */}
          {/* The ReUI sidebar owns its trigger inside the app-shell header.
              Keep the provider flag so page headers do not add a second
              fallback trigger inside the canvas. */}
          <SidebarProvider
            hasExternalTrigger
            hoverReveal
            compactBehavior="collapse"
            glass
            data-native-vibrancy={usesNativeVibrancy ? "true" : undefined}
            className={cn(
              "flex-1 [--sidebar-width:260px] [--sidebar-border:transparent] [--sidebar-wrapper-fill:var(--app-shell)]",
              "[&_[data-slot=sidebar-inner]]:border-border/80 [&_[data-slot=sidebar-inner]]:border",
              "[&_[data-slot=sidebar-inner]]:shadow-xs [&_[data-slot=sidebar-inner]]:shadow-black/5",
              "[&_[data-slot=sidebar-menu-button][data-active]]:border-border/60! [&_[data-slot=sidebar-menu-button][data-active]]:border",
              "[&_[data-slot=sidebar-menu-button][data-active]]:shadow-xs! [&_[data-slot=sidebar-menu-button][data-active]]:shadow-black/5!",
              "[&_[data-slot=sidebar-menu-button][data-active]]:bg-background! [&_[data-slot=sidebar-menu-button][data-active]]:hover:bg-background! **:data-[slot=sidebar-menu-button]:hover:bg-transparent!",
              "[&_[data-slot=sidebar-menu-button][data-active]]:text-foreground [&_[data-slot=sidebar-menu-button][data-active]>svg]:text-primary [&_[data-slot=sidebar-menu-button][data-active]>svg]:opacity-100",
              "**:data-[slot=sidebar-menu-button]:text-accent-foreground/80 **:data-[slot=sidebar-menu-button]:hover:text-foreground",
              "[&_[data-collapsible=icon]_[data-slot=sidebar-menu-button][data-active]>svg]:-ml-px",
              "[&_[data-slot=sidebar-menu-button]:hover>svg]:opacity-100 [&_[data-slot=sidebar-menu-button]>svg]:opacity-60",
              "[&_[data-slot=sidebar-menu-sub-button][data-active]]:border-border/60! [&_[data-slot=sidebar-menu-sub-button][data-active]]:border",
              "[&_[data-slot=sidebar-menu-sub-button][data-active]]:shadow-xs! [&_[data-slot=sidebar-menu-sub-button][data-active]]:shadow-black/5!",
              "[&_[data-slot=sidebar-menu-sub-button][data-active]]:bg-background! [&_[data-slot=sidebar-menu-sub-button][data-active]]:hover:bg-background! **:data-[slot=sidebar-menu-sub-button]:hover:bg-transparent!",
              "[&_[data-slot=sidebar-menu-sub-button][data-active]]:text-foreground [&_[data-slot=sidebar-menu-sub-button][data-active]>svg]:text-primary [&_[data-slot=sidebar-menu-sub-button][data-active]>svg]:opacity-100",
              "**:data-[slot=sidebar-menu-sub-button]:text-accent-foreground/80 **:data-[slot=sidebar-menu-sub-button]:hover:text-foreground",
              "[&_[data-slot=sidebar-menu-sub-button]:hover>svg]:opacity-100 [&_[data-slot=sidebar-menu-sub-button]>svg]:opacity-60",
              usesNativeVibrancy ? "bg-transparent" : "bg-app-shell",
            )}
          >
            {slug && <GlobalShortcuts />}
            {slug && <WindowToolbar />}
            {slug && (
              <AppSidebar
                topSlot={<SidebarTopSpacer />}
                searchSlot={<SearchTrigger />}
              />
            )}
            {/* Right side: header + content container */}
            <div className="flex flex-1 min-w-0 flex-col">
              <MainTopBar />
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
                {slug && <FloatingChat />}
              </MainCanvas>
            </div>
          </SidebarProvider>
        </div>
        {slug && <ModalRegistry />}
        {slug && <SearchCommand />}
        <WindowOverlay />
      </WorkspaceSlugProvider>
    </DesktopNavigationProvider>
  );
}
