"use client"

import { useEffect, useState, type MouseEvent, type ReactNode } from "react"
import { useTheme } from "next-themes"

import { cn } from "@orvilo/ui/lib/utils"
import {
  Avatar,
  AvatarFallback,
  AvatarImage,
} from "@orvilo/ui/components/ui/avatar"
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuGroup,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuRadioGroup,
  DropdownMenuRadioItem,
  DropdownMenuSeparator,
  DropdownMenuSub,
  DropdownMenuSubContent,
  DropdownMenuSubTrigger,
  DropdownMenuTrigger,
} from "@orvilo/ui/components/ui/dropdown-menu"
import {
  SidebarMenu,
  SidebarMenuButton,
  SidebarMenuItem,
  useSidebar,
} from "@orvilo/ui/components/ui/sidebar"
import { USER, WORKSPACE_LOGOS, WORKSPACES, type Workspace } from "./data"
import {
  SunIcon,
  MoonIcon,
  MonitorIcon,
  CheckIcon,
  MoreHorizontalIcon,
  PlusIcon,
  UserIcon,
  CreditCardIcon,
  SettingsIcon,
  PaletteIcon,
  LogOutIcon,
} from "lucide-react"

export type NavWorkspaceUser = {
  name: string
  initials: string
  email?: string | null
  avatarUrl?: string | null
}

export type NavWorkspaceLabels = {
  menuAriaLabel: string
  workspaces: string
  createWorkspace: string
  account: string
  profile: string
  billing: string
  preferences: string
  theme: string
  signOut: string
  light: string
  dark: string
  system: string
}

export type NavWorkspaceProps = {
  user?: NavWorkspaceUser
  workspaces?: Workspace[]
  activeWorkspace?: Workspace
  labels?: Partial<NavWorkspaceLabels>
  workspaceMenuContent?: ReactNode
  onSelectWorkspace?: (workspaceId: string, event: MouseEvent<HTMLElement>) => void
  onCreateWorkspace?: () => void
  onProfile?: () => void
  onBilling?: () => void
  onPreferences?: () => void
  onSignOut?: () => void
  onOpenChange?: (open: boolean) => void
}

const DEFAULT_LABELS: NavWorkspaceLabels = {
  menuAriaLabel: "Open workspace menu",
  workspaces: "Workspaces",
  createWorkspace: "New workspace",
  account: "Account",
  profile: "Profile",
  billing: "Billing",
  preferences: "Preferences",
  theme: "Theme",
  signOut: "Sign out",
  light: "Light",
  dark: "Dark",
  system: "System",
}

const THEMES = [
  { value: "light", icon: <SunIcon className="size-3.5" aria-hidden="true" /> },
  { value: "dark", icon: <MoonIcon className="size-3.5" aria-hidden="true" /> },
  { value: "system", icon: <MonitorIcon className="size-3.5" aria-hidden="true" /> },
] as const

function ThemeSubmenu({ labels }: { labels: NavWorkspaceLabels }) {
  const { theme, setTheme } = useTheme()
  const [mounted, setMounted] = useState(false)

  useEffect(() => {
    setMounted(true)
  }, [])

  const currentTheme = mounted ? (theme ?? "system") : "system"
  const themeLabels = { light: labels.light, dark: labels.dark, system: labels.system }

  return (
    <DropdownMenuSub>
      <DropdownMenuSubTrigger>
        <PaletteIcon aria-hidden="true" />
        {labels.theme}
      </DropdownMenuSubTrigger>
      <DropdownMenuSubContent>
        <DropdownMenuRadioGroup
          value={currentTheme}
          onValueChange={(value) => setTheme(value)}
        >
          {THEMES.map(({ value, icon }) => (
            <DropdownMenuRadioItem key={value} value={value}>
              {icon}
              {themeLabels[value]}
            </DropdownMenuRadioItem>
          ))}
        </DropdownMenuRadioGroup>
      </DropdownMenuSubContent>
    </DropdownMenuSub>
  )
}

function WorkspaceAvatar({
  workspace,
  className,
}: {
  workspace: Workspace
  className?: string
}) {
  const LogoComponent = WORKSPACE_LOGOS[workspace.id]
  const fallback = (
    <AvatarFallback
      className={cn(
        "border text-sm font-medium",
        workspace.avatarClassName ?? "border-border bg-muted text-muted-foreground",
      )}
    >
      {workspace.name.charAt(0).toUpperCase()}
    </AvatarFallback>
  )

  if (workspace.imageUrl) {
    return (
      <Avatar className={cn("shrink-0", className)}>
        <AvatarImage src={workspace.imageUrl} alt={workspace.name} />
        {fallback}
      </Avatar>
    )
  }

  if (LogoComponent) return <LogoComponent className={cn("shrink-0", className)} />

  return <Avatar className={cn("shrink-0", className)}>{fallback}</Avatar>
}

function WorkspaceItem({
  workspace,
  isActive,
  onSelect,
}: {
  workspace: Workspace
  isActive: boolean
  onSelect: (id: string, event: MouseEvent<HTMLElement>) => void
}) {
  return (
    <DropdownMenuItem
      onClick={(event) => onSelect(workspace.id, event)}
      onAuxClick={(event) => {
        if (event.button === 1) onSelect(workspace.id, event)
      }}
    >
      <WorkspaceAvatar workspace={workspace} className="size-5!" />
      <span className="flex-1 truncate text-sm font-medium">{workspace.name}</span>
      {workspace.hasUnread && !isActive ? (
        <span
          data-slot="workspace-row-unread-dot"
          className="ml-auto size-2 shrink-0 rounded-full bg-primary"
        />
      ) : null}
      {isActive ? (
        <CheckIcon className="ml-auto size-3.5 shrink-0 opacity-60" aria-hidden="true" />
      ) : null}
    </DropdownMenuItem>
  )
}

export function NavWorkspace({
  user,
  workspaces = WORKSPACES,
  activeWorkspace: activeWorkspaceProp,
  labels,
  workspaceMenuContent,
  onSelectWorkspace,
  onCreateWorkspace,
  onProfile,
  onBilling,
  onPreferences,
  onSignOut,
  onOpenChange,
}: NavWorkspaceProps = {}) {
  const [localWorkspaceId, setLocalWorkspaceId] = useState(workspaces[0]?.id ?? "")
  const { isCompact } = useSidebar()
  const copy = { ...DEFAULT_LABELS, ...labels }
  const resolvedUser: NavWorkspaceUser = user ?? {
    name: USER.name,
    initials: USER.initials,
    email: USER.email,
    avatarUrl: USER.avatar,
  }
  const activeWorkspace =
    activeWorkspaceProp ??
    workspaces.find((workspace) => workspace.id === localWorkspaceId) ??
    workspaces[0]
  const selectWorkspace =
    onSelectWorkspace ?? ((workspaceId: string) => setLocalWorkspaceId(workspaceId))

  if (!activeWorkspace) return null

  return (
    <SidebarMenu>
      <SidebarMenuItem>
        <DropdownMenu onOpenChange={onOpenChange}>
          <DropdownMenuTrigger
            className="-ml-1 bg-transparent pr-0! group-data-[collapsible=icon]:-ml-1!"
            render={<SidebarMenuButton aria-label={copy.menuAriaLabel} />}
          >
            <div className="relative flex min-w-0 flex-1 items-center gap-2">
              <Avatar className="size-7 shrink-0 rounded-md in-data-[state=collapsed]:size-6!">
                {resolvedUser.avatarUrl ? (
                  <AvatarImage
                    src={resolvedUser.avatarUrl}
                    alt={resolvedUser.name}
                    className="rounded-md"
                  />
                ) : null}
                <AvatarFallback className="text-[10px] font-semibold">
                  {resolvedUser.initials}
                </AvatarFallback>
              </Avatar>
              <div className="flex min-w-0 flex-col in-data-[state=collapsed]:hidden">
                <span className="truncate text-xs font-medium text-sidebar-text-primary">
                  {resolvedUser.name}
                </span>
                <span className="inline-flex items-center gap-1">
                  <WorkspaceAvatar workspace={activeWorkspace} className="size-3!" />
                  <span className="truncate text-[10px] text-sidebar-text-secondary">
                    {activeWorkspace.name}
                  </span>
                </span>
              </div>
              {activeWorkspace.hasUnread ? (
                <span
                  data-slot="workspace-unread-dot"
                  className="absolute -top-0.5 left-5 size-2 rounded-full bg-primary ring-1 ring-sidebar"
                />
              ) : null}
            </div>
            <MoreHorizontalIcon
              className="mr-1 ml-auto size-4 shrink-0 opacity-50 in-data-[state=collapsed]:hidden"
              aria-hidden="true"
            />
          </DropdownMenuTrigger>

          <DropdownMenuContent
            side={isCompact ? "top" : "right"}
            align="end"
            sideOffset={8}
            className="w-60"
          >
            <DropdownMenuGroup>
              <DropdownMenuLabel className="text-xs font-normal text-muted-foreground">
                {copy.workspaces}
              </DropdownMenuLabel>
              {workspaces.map((workspace) => (
                <WorkspaceItem
                  key={workspace.id}
                  workspace={workspace}
                  isActive={activeWorkspace.id === workspace.id}
                  onSelect={selectWorkspace}
                />
              ))}
              {onCreateWorkspace ? (
                <DropdownMenuItem onClick={onCreateWorkspace}>
                  <PlusIcon aria-hidden="true" className="mx-0.5" />
                  {copy.createWorkspace}
                </DropdownMenuItem>
              ) : null}
              {workspaceMenuContent}
            </DropdownMenuGroup>

            <DropdownMenuSeparator />

            <DropdownMenuGroup>
              <DropdownMenuLabel className="text-xs font-normal text-muted-foreground">
                {copy.account}
              </DropdownMenuLabel>
              {resolvedUser.email ? (
                <div className="truncate px-2 pb-1 text-xs text-muted-foreground">
                  {resolvedUser.email}
                </div>
              ) : null}
              <DropdownMenuItem onClick={onProfile}>
                <UserIcon aria-hidden="true" />
                {copy.profile}
              </DropdownMenuItem>
              {onBilling ? (
                <DropdownMenuItem onClick={onBilling}>
                  <CreditCardIcon aria-hidden="true" />
                  {copy.billing}
                </DropdownMenuItem>
              ) : null}
              <DropdownMenuItem onClick={onPreferences}>
                <SettingsIcon aria-hidden="true" />
                {copy.preferences}
              </DropdownMenuItem>
              <ThemeSubmenu labels={copy} />
            </DropdownMenuGroup>

            <DropdownMenuSeparator />

            <DropdownMenuGroup>
              <DropdownMenuItem onClick={onSignOut}>
                <LogOutIcon aria-hidden="true" />
                {copy.signOut}
              </DropdownMenuItem>
            </DropdownMenuGroup>
          </DropdownMenuContent>
        </DropdownMenu>
      </SidebarMenuItem>
    </SidebarMenu>
  )
}
