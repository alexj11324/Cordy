"use client"

import { useEffect, type CSSProperties, type ReactNode } from "react"

import { cn } from "@patchbay/ui/lib/utils"
import {
  SidebarInset,
  SidebarProvider,
  useSidebar,
} from "@patchbay/ui/components/ui/sidebar"

import { AppSidebar } from "./app-sidebar"

// ── Theme tokens ──
// CSS custom properties for the dark-zinc sidebar palette.
// Swap any value here to retheme the sidebar across desktop and mobile.

const SIDEBAR_STYLE = {
  "--sidebar-width": "250px",
  "--sidebar-width-icon": "50px",
  "--header-height": "50px",
  "--sidebar": "var(--color-zinc-900)",
  "--sidebar-foreground": "var(--color-zinc-100)",
  "--sidebar-border":
    "color-mix(in oklab, var(--color-zinc-700) 52%, transparent)",
  "--sidebar-accent": "var(--color-zinc-800)",
  "--sidebar-accent-foreground": "var(--color-zinc-100)",
  "--sidebar-primary": "var(--color-zinc-100)",
  "--sidebar-primary-foreground": "var(--color-zinc-900)",
  "--sidebar-ring": "var(--color-zinc-400)",
}

// ── Behavioral classes ──
// One entry per concern. Edit here to change interactive states globally.

const SIDEBAR_THEME_CLASSES = [
  // Idle + hover nav buttons
  "[&_[data-slot=sidebar-menu-button]]:text-zinc-400 [&_[data-slot=sidebar-menu-button]>svg]:opacity-55 [&_[data-slot=sidebar-menu-button]:hover]:bg-zinc-800/90 [&_[data-slot=sidebar-menu-button]:hover]:text-zinc-100 [&_[data-slot=sidebar-menu-button]:hover>svg]:opacity-100",
  // Active nav buttons
  "[&_[data-slot=sidebar-menu-button][data-active]]:bg-zinc-800 [&_[data-slot=sidebar-menu-button][data-active]]:text-zinc-50 [&_[data-slot=sidebar-menu-button][data-active]>svg]:opacity-100",
  // Idle + hover sub-nav buttons
  "[&_[data-slot=sidebar-menu-sub-button]]:text-zinc-500 [&_[data-slot=sidebar-menu-sub-button]:hover]:bg-zinc-800/80 [&_[data-slot=sidebar-menu-sub-button]:hover]:text-zinc-100",
  // Active sub-nav buttons
  "[&_[data-slot=sidebar-menu-sub-button][data-active]]:bg-zinc-800 [&_[data-slot=sidebar-menu-sub-button][data-active]]:text-zinc-50",
  // Group labels
  "[&_[data-slot=sidebar-group-label]]:text-zinc-500",
] as const

// Pre-split into individual tokens for classList.add / classList.remove
const SIDEBAR_CLASS_TOKENS = SIDEBAR_THEME_CLASSES.flatMap((c) => c.split(" "))

// ── useMobileBodyTheme ──
// On mobile, the sidebar Sheet is portalled to <body> - outside SidebarProvider's
// subtree. This hook applies the theme directly to document.body on mobile and
// cleans up when the viewport returns to desktop.

function useMobileBodyTheme(isMobile: boolean) {
  useEffect(() => {
    if (!isMobile) return

    const body = document.body

    body.classList.add(...SIDEBAR_CLASS_TOKENS)

    const prev: Record<string, string> = {}
    for (const [prop, value] of Object.entries(SIDEBAR_STYLE)) {
      prev[prop] = body.style.getPropertyValue(prop)
      body.style.setProperty(prop, value)
    }

    return () => {
      body.classList.remove(...SIDEBAR_CLASS_TOKENS)
      for (const [prop, prevValue] of Object.entries(prev)) {
        if (prevValue) body.style.setProperty(prop, prevValue)
        else body.style.removeProperty(prop)
      }
    }
  }, [isMobile])
}

// ── SidebarBody ──
// Wraps SidebarInset and activates the mobile body theme.

function SidebarBody({ children }: { children: ReactNode }) {
  const { isCompact: isMobile } = useSidebar()
  useMobileBodyTheme(isMobile)

  return (
    <SidebarInset className="ml-0! overflow-y-auto">{children}</SidebarInset>
  )
}

// ── SidebarShell ──
// Top-level layout shell - composes the themed SidebarProvider, AppSidebar,
// and the page body.

export function SidebarShell({ children }: { children: ReactNode }) {
  return (
    <SidebarProvider
      className={cn("h-screen", ...SIDEBAR_THEME_CLASSES)}
      style={SIDEBAR_STYLE as CSSProperties}
    >
      {/* Sidebar */}
      <AppSidebar />
      <SidebarBody>{children}</SidebarBody>
    </SidebarProvider>
  )
}
