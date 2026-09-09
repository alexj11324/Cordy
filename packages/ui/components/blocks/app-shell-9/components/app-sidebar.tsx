"use client"

import { cn } from "@patchbay/ui/lib/utils"
import {
  Sidebar,
  SidebarContent,
  SidebarFooter,
  SidebarHeader,
  useSidebar,
} from "@patchbay/ui/components/ui/sidebar"

import { Logo } from "./logo"
import { NavMain } from "./nav-main"
import { NavTeamMembers } from "./nav-team-members"
import { NavWorkspace } from "./nav-workspace"
import { NotificationsPopover } from "./notifications-popover"
import { SearchForm } from "./search-form"
import { UpgradeCard } from "./upgrade-card"

// ── Sidebar Rail Toggle ──

function SidebarRailToggle() {
  const { state, toggleSidebar } = useSidebar()
  const isExpanded = state === "expanded"

  return (
    <button
      type="button"
      aria-label={isExpanded ? "Collapse sidebar" : "Expand sidebar"}
      onClick={toggleSidebar}
      style={{
        left: isExpanded
          ? "var(--sidebar-width)"
          : "calc(var(--sidebar-width-icon) + 16px)",
      }}
      className={cn(
        // Hidden below md: on mobile the sidebar is a Sheet (see the header's
        // md:hidden trigger), so this fixed rail toggle would otherwise float
        // over the page content in the middle of the screen.
        "group/rail fixed top-1/2 z-30 hidden h-12 w-7 -translate-y-1/2 cursor-pointer items-center pl-2 outline-none md:flex",
        "transition-[left] duration-200 ease-linear motion-reduce:transition-none"
      )}
    >
      <span className="flex flex-col items-center">
        <span
          aria-hidden="true"
          className={cn(
            "bg-foreground/40 block h-2 w-0.5",
            "rounded-t-full",
            "origin-bottom transition-all duration-100 ease-linear",
            isExpanded
              ? "group-hover/rail:bg-foreground/60 group-hover/rail:rotate-40"
              : "group-hover/rail:bg-foreground/60 group-hover/rail:-rotate-40"
          )}
        />
        <span
          aria-hidden="true"
          className={cn(
            "bg-foreground/40 block h-2 w-0.5",
            "rounded-b-full",
            "origin-top transition-all duration-100 ease-linear",
            isExpanded
              ? "group-hover/rail:bg-foreground/60 group-hover/rail:-rotate-40"
              : "group-hover/rail:bg-foreground/60 group-hover/rail:rotate-40"
          )}
        />
      </span>
      <span
        className={cn(
          "border-border bg-foreground text-background absolute left-full -ml-2 border px-2 py-0.5 text-[11px] font-medium whitespace-nowrap shadow-xs shadow-black/5",
          "rounded-md",
          "pointer-events-none transition-all duration-200 ease-out",
          "-translate-x-0.5 opacity-0",
          "group-hover/rail:translate-x-0 group-hover/rail:opacity-100"
        )}
      >
        {isExpanded ? "Collapse" : "Expand"}
      </span>
    </button>
  )
}

// ── App Sidebar ──

export function AppSidebar() {
  return (
    <>
      <Sidebar collapsible="icon" variant="inset" className="dark">
        {/* Header */}
        <SidebarHeader className="flex flex-row items-center justify-between in-data-[state=collapsed]:flex-col in-data-[state=collapsed]:items-start in-data-[state=collapsed]:justify-center">
          <div className="inline-flex min-h-10 items-center gap-2 px-0.5 transition-all duration-200 ease-linear">
            <Logo className="text-white" />
            <span className="text-sm font-medium text-zinc-100 in-data-[state=collapsed]:hidden">
              ReUI
            </span>
          </div>

          <div className="inline-flex items-center gap-0.5 in-data-[state=collapsed]:flex-col">
            <NotificationsPopover />
          </div>
        </SidebarHeader>

        {/* Sidebar */}
        <SidebarContent>
          <div className="py-2">
            <SearchForm />
          </div>

          <NavMain />
          <NavTeamMembers />

          <div className="mt-auto">
            <UpgradeCard />
          </div>
        </SidebarContent>

        {/* Footer */}
        <SidebarFooter className="pb-2">
          <NavWorkspace />
        </SidebarFooter>
      </Sidebar>

      {/* Rendered outside the dark sidebar so the rail tick follows the content theme (visible on the light seam) and is not clipped by the sidebar overflow. */}
      <SidebarRailToggle />
    </>
  )
}