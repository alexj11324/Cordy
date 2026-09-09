"use client";

import type { ReactNode } from "react";
import { cn } from "@orvilo/ui/lib/utils";
import { SidebarProvider, SidebarInset } from "@orvilo/ui/components/ui/sidebar";
import { ModalRegistry } from "../modals/registry";
import { SourceBackfillModal } from "../onboarding";
import { AppSidebar } from "./app-sidebar";
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
        className={cn(
          "h-svh [--sidebar-width:260px] [--sidebar-border:transparent]",
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
        )}
      >
        <GlobalShortcuts />
        <WorkspacePresencePrefetch />
        <AppSidebar searchSlot={searchSlot} />
        <SidebarInset className="relative overflow-hidden">
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
        </SidebarInset>
      </SidebarProvider>
    </DashboardGuard>
  );
}
