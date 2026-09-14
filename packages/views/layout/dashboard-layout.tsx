"use client";

import type { ReactNode } from "react";
import { cn } from "@orvilo/ui/lib/utils";
import {
  SidebarInset,
  SidebarProvider,
  SidebarTrigger,
} from "@orvilo/ui/components/ui/sidebar";
import { ModalRegistry } from "../modals/registry";
import { SourceBackfillModal } from "../onboarding";
import { GlobalRightSidebar, GlobalRightSidebarToggle } from "../chat/global-right-sidebar";
import { AppSidebar } from "./app-sidebar";
import { ShellBreadcrumb } from "./shell-breadcrumb";
import {
  ShellHeaderActionsSlot,
  ShellHeaderProvider,
} from "./shell-header";
import { DashboardGuard } from "./dashboard-guard";
import { NavigationProgress } from "./navigation-progress";
import { WorkspacePresencePrefetch } from "./workspace-presence-prefetch";
import { GlobalShortcuts } from "./global-shortcuts";
import { AgentThreadPanelLayout } from "../agent-thread/components/agent-thread-panel-layout";

interface DashboardLayoutProps {
  children: ReactNode;
  /** Rendered inside SidebarInset (e.g. ChatWindow, ChatFab — absolute-positioned overlays) */
  extra?: ReactNode;
  /** Rendered inside sidebar header as a search trigger */
  searchSlot?: ReactNode;
  /** Loading indicator */
  loadingIndicator?: ReactNode;
}

export function DashboardLayout({
  children,
  extra,
  searchSlot,
  loadingIndicator,
}: DashboardLayoutProps) {
  return (
    <DashboardGuard
      loadingFallback={
        <div className="flex h-svh items-center justify-center">
          {loadingIndicator}
        </div>
      }
    >
      <SidebarProvider
        hasExternalTrigger
        className={cn(
          // `--sidebar-width` only. The shell used to rebind `--sidebar`,
          // `--sidebar-accent` and `--sidebar-accent-foreground` here, to
          // Pulse Help Desk's values: the rail took `--background` and the
          // accents took a 5% primary wash. `tokens.css` carries LobeHub's
          // palette for those tokens app-wide now, and the settings dialog
          // portals out of this subtree, so a rebind here made the rail behind
          // the dialog and the dialog's own rail read two different values —
          // the seam this change exists to remove. Measured before removing:
          // the app rail painted `--color-background` (white) while the
          // settings rail painted `--sidebar` (#f8f8f8).
          "h-svh [--sidebar-width:260px]",
        )}
      >
        <ShellHeaderProvider>
          <GlobalShortcuts />
          <WorkspacePresencePrefetch />
          <AppSidebar searchSlot={searchSlot} />
          <SidebarInset className="relative m-0! flex-row! overflow-hidden">
            <div className="relative flex min-h-0 min-w-0 flex-1 flex-col overflow-hidden" data-slot="shell-main-column">
            <header className="flex h-12 shrink-0 items-center gap-2 px-4">
              <SidebarTrigger className="xl:hidden" />
              <div className="min-w-0 flex-1">
                <ShellBreadcrumb />
              </div>
              <ShellHeaderActionsSlot />
              <GlobalRightSidebarToggle />
            </header>
            <NavigationProgress />
            <AgentThreadPanelLayout>
              <div
                className="flex min-h-0 min-w-0 flex-1 flex-col overflow-y-auto overscroll-contain"
                data-testid="web-route-scroll-viewport"
              >
                {children}
              </div>
            </AgentThreadPanelLayout>
            <ModalRegistry />
            <SourceBackfillModal />
            {extra}
            </div>
            <GlobalRightSidebar />
          </SidebarInset>
        </ShellHeaderProvider>
      </SidebarProvider>
    </DashboardGuard>
  );
}
