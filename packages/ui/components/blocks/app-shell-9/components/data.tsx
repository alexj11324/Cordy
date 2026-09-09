import { type ComponentType, type ReactNode } from "react"
import { LayoutDashboardIcon, FolderIcon, SquareCheckIcon, CircleDotIcon, UsersIcon, BarChart3Icon, UserIcon, MessageSquareIcon, UserMinusIcon } from "lucide-react"

// ── Types ──

export type NavChild = {
  id: string
  label: string
  href: string
  isActive?: boolean
}

export type NavItem = {
  id: string
  label: string
  href?: string
  icon: ReactNode
  badge?: string | number
  isActive?: boolean
  children?: NavChild[]
}

export type TeamMember = {
  id: string
  name: string
  role: string
  href: string
  avatar: string
  initials: string
  status: "online" | "away" | "offline"
}

export type TeamMemberAction = {
  id: string
  label: string
  icon: ReactNode
  destructive: boolean
}

export type Workspace = {
  id: string
  name: string
  tier?: string
  imageUrl?: string
  avatarClassName?: string
  hasUnread?: boolean
}

export type NotificationType =
  | "mention"
  | "comment"
  | "share"
  | "invite"
  | "billing"
  | "security"
  | "feature"
  | "deployment"
  | "usage"
  | "system"
  | "task"
  | "approval"
  | "integration"
  | "achievement"
  | "feedback"
  | "team_join"
  | "reaction"
  | "review"
  | "event"

export type NotificationVariant = "info" | "success" | "warning" | "destructive"

export type NotificationAction = {
  label: string
  variant?: "default" | "outline" | "destructive"
}

export type NotificationAttachment = {
  name: string
  size: string
}

export type NotificationAvatar = {
  src: string
  fallback: string
}

export type NotificationGroupMember = {
  src: string
  fallback: string
  online?: boolean
}

export type NotificationMeta = {
  label: string
  value: string
  color?: string
}

export type Notification = {
  id: string
  type: NotificationType
  variant?: NotificationVariant
  title: string
  body?: string
  time: string
  unread?: boolean
  avatar?: NotificationAvatar
  username?: string
  link?: string
  badge?: string
  actions?: NotificationAction[]
  attachment?: NotificationAttachment
  meta?: NotificationMeta
  progress?: number
  progressVariant?: "default" | "success"
  avatarGroup?: NotificationGroupMember[]
  avatarGroupCount?: number
  rating?: number
  eventDate?: string
  eventTime?: string
}

// ── Workspace Logos ──

function AcmeLogo({ className }: { className?: string }) {
  return (
    <svg
      viewBox="0 0 28 28"
      fill="none"
      width="100%"
      height="100%"
      className={className}
      aria-hidden="true"
    >
      <defs>
        <linearGradient
          id="sb9-acme"
          x1="0"
          y1="0"
          x2="28"
          y2="28"
          gradientUnits="userSpaceOnUse"
        >
          <stop offset="0%" stopColor="#6366f1" />
          <stop offset="50%" stopColor="#8b5cf6" />
          <stop offset="100%" stopColor="#ec4899" />
        </linearGradient>
      </defs>
      <circle cx="14" cy="14" r="14" fill="url(#sb9-acme)" />
    </svg>
  )
}

function VercelLogo({ className }: { className?: string }) {
  return (
    <svg
      viewBox="0 0 28 28"
      fill="none"
      width="100%"
      height="100%"
      className={className}
      aria-hidden="true"
    >
      <defs>
        <linearGradient
          id="sb9-vercel"
          x1="0"
          y1="0"
          x2="28"
          y2="28"
          gradientUnits="userSpaceOnUse"
        >
          <stop offset="0%" stopColor="#0ea5e9" />
          <stop offset="50%" stopColor="#06b6d4" />
          <stop offset="100%" stopColor="#10b981" />
        </linearGradient>
      </defs>
      <circle cx="14" cy="14" r="14" fill="url(#sb9-vercel)" />
    </svg>
  )
}

function OpenAILogo({ className }: { className?: string }) {
  return (
    <svg
      viewBox="0 0 28 28"
      fill="none"
      width="100%"
      height="100%"
      className={className}
      aria-hidden="true"
    >
      <defs>
        <linearGradient
          id="sb9-openai"
          x1="0"
          y1="0"
          x2="28"
          y2="28"
          gradientUnits="userSpaceOnUse"
        >
          <stop offset="0%" stopColor="#f97316" />
          <stop offset="50%" stopColor="#f59e0b" />
          <stop offset="100%" stopColor="#84cc16" />
        </linearGradient>
      </defs>
      <circle cx="14" cy="14" r="14" fill="url(#sb9-openai)" />
    </svg>
  )
}

// ── Workspaces ──

export const WORKSPACES: Workspace[] = [
  { id: "acme", name: "Acme Corp", tier: "Enterprise" },
  { id: "vercel", name: "Vercel", tier: "Pro" },
  { id: "openai", name: "OpenAI", tier: "Team" },
]

export const WORKSPACE_LOGOS: Record<
  string,
  ComponentType<{ className?: string }>
> = {
  acme: AcmeLogo,
  vercel: VercelLogo,
  openai: OpenAILogo,
}

// ── Nav Main ──

export const NAV_MAIN: NavItem[] = [
  {
    id: "dashboard",
    label: "Dashboard",
    href: "/dashboard",
    icon: (
      <LayoutDashboardIcon aria-hidden="true" />
    ),
    isActive: true,
  },
  {
    id: "projects",
    label: "Projects",
    href: "/projects",
    icon: (
      <FolderIcon aria-hidden="true" />
    ),
  },
  {
    id: "tasks",
    label: "My Tasks",
    href: "/tasks",
    icon: (
      <SquareCheckIcon aria-hidden="true" />
    ),
  },
  {
    id: "issues",
    label: "Issues",
    icon: (
      <CircleDotIcon aria-hidden="true" />
    ),
    children: [
      { id: "backlog", label: "Backlog", href: "/issues/backlog" },
      { id: "in-progress", label: "In Progress", href: "/issues/in-progress" },
      { id: "resolved", label: "Resolved", href: "/issues/resolved" },
    ],
  },
  {
    id: "team",
    label: "Team",
    icon: (
      <UsersIcon aria-hidden="true" />
    ),
    children: [
      { id: "members", label: "Members", href: "/team/members" },
      { id: "workload", label: "Workload", href: "/team/workload" },
      { id: "permissions", label: "Permissions", href: "/team/permissions" },
    ],
  },
  {
    id: "reports",
    label: "Reports",
    href: "/reports",
    icon: (
      <BarChart3Icon aria-hidden="true" />
    ),
  },
]

// ── Team Members ──

export const TEAM_MEMBERS: TeamMember[] = [
  {
    id: "tm1",
    name: "Daniel Parker",
    role: "Product Lead",
    href: "/team/daniel-parker",
    avatar:
      "https://images.unsplash.com/photo-1500648767791-00dcc994a43e?w=96&h=96&dpr=2&q=80",
    initials: "DP",
    status: "online",
  },
  {
    id: "tm2",
    name: "Matthew Reed",
    role: "Engineering Lead",
    href: "/team/matthew-reed",
    avatar:
      "https://images.unsplash.com/photo-1472099645785-5658abf4ff4e?w=96&h=96&dpr=2&q=80",
    initials: "MR",
    status: "online",
  },
  {
    id: "tm3",
    name: "Ryan Brooks",
    role: "Frontend Dev",
    href: "/team/ryan-brooks",
    avatar:
      "https://images.unsplash.com/photo-1507003211169-0a1dd7228f2d?w=96&h=96&dpr=2&q=80",
    initials: "RB",
    status: "away",
  },
  {
    id: "tm4",
    name: "Lena Dawson",
    role: "Backend Dev",
    href: "/team/lena-dawson",
    avatar:
      "https://images.unsplash.com/photo-1519699047748-de8e457a634e?w=96&h=96&dpr=2&q=80",
    initials: "LD",
    status: "online",
  },
  {
    id: "tm5",
    name: "Sophie Bennett",
    role: "UX Designer",
    href: "/team/sophie-bennett",
    avatar:
      "https://images.unsplash.com/photo-1438761681033-6461ffad8d80?w=96&h=96&dpr=2&q=80",
    initials: "SB",
    status: "online",
  },
  {
    id: "tm6",
    name: "Liam Carter",
    role: "QA Engineer",
    href: "/team/liam-carter",
    avatar:
      "https://images.unsplash.com/photo-1573496359142-b8d87734a5a2?w=96&h=96&dpr=2&q=80",
    initials: "LC",
    status: "offline",
  },
]

// ── Team Member Actions ──

export const TEAM_MEMBER_ACTIONS: TeamMemberAction[] = [
  {
    id: "profile",
    label: "View Profile",
    icon: (
      <UserIcon aria-hidden="true" />
    ),
    destructive: false,
  },
  {
    id: "message",
    label: "Send Message",
    icon: (
      <MessageSquareIcon aria-hidden="true" />
    ),
    destructive: false,
  },
  {
    id: "assign",
    label: "Assign Task",
    icon: (
      <SquareCheckIcon aria-hidden="true" />
    ),
    destructive: false,
  },
  {
    id: "remove",
    label: "Remove from Team",
    icon: (
      <UserMinusIcon aria-hidden="true" />
    ),
    destructive: true,
  },
]

// ── User ──

export const USER = {
  name: "Nick Bold",
  email: "nick@reui.io",
  avatar:
    "https://images.unsplash.com/photo-1543299750-19d1d6297053?w=96&h=96&dpr=2&q=80",
  initials: "NB",
} as const

// ── Notifications ──

export const NOTIFICATIONS: Notification[] = [
  {
    id: "n1",
    type: "mention",
    title: "mentioned you in",
    body: '"Can you review the changes?"',
    time: "2m ago",
    unread: false,
    avatar: {
      src: "https://images.unsplash.com/photo-1494790108377-be9c29b29330?w=96&h=96&dpr=2&q=80",
      fallback: "SR",
    },
    username: "@sarah_smith",
    link: "#PR-1024",
  },
  {
    id: "n2",
    type: "approval",
    variant: "warning",
    title: "Pending approval",
    body: "Design System v2.0 release requires your approval before deployment.",
    time: "5m ago",
    unread: true,
    actions: [
      { label: "Approve", variant: "default" },
      { label: "Review", variant: "outline" },
    ],
    meta: { label: "Priority", value: "High", color: "text-warning" },
  },
  {
    id: "n3",
    type: "share",
    title: "shared",
    body: "Project Timeline",
    time: "15m ago",
    unread: true,
    avatar: {
      src: "https://images.unsplash.com/photo-1485206412256-701ccc5b93ca?w=96&h=96&dpr=2&q=80",
      fallback: "MA",
    },
    username: "@maverick",
    attachment: { name: "project-plan.pdf", size: "2mb" },
  },
  {
    id: "n4",
    type: "task",
    variant: "info",
    title: "Task assigned to you",
    body: "Implement user authentication flow for the mobile app.",
    time: "30m ago",
    unread: true,
    meta: { label: "Due", value: "Tomorrow", color: "text-destructive" },
  },
  {
    id: "n5",
    type: "team_join",
    variant: "success",
    title: "4 people joined your workspace",
    body: "Sarah, Mike, Emma and James just joined ReUI Pro.",
    time: "45m ago",
    unread: true,
    avatarGroup: [
      {
        src: "https://images.unsplash.com/photo-1519699047748-de8e457a634e?w=96&h=96&dpr=2&q=80",
        fallback: "SC",
        online: true,
      },
      {
        src: "https://images.unsplash.com/photo-1584308972272-9e4e7685e80f?w=96&h=96&dpr=2&q=80",
        fallback: "MR",
      },
      {
        src: "https://images.unsplash.com/photo-1485893086445-ed75865251e0?w=96&h=96&dpr=2&q=80",
        fallback: "EW",
      },
    ],
    avatarGroupCount: 1,
  },
  {
    id: "n6",
    type: "invite",
    variant: "info",
    title: "Team Invitation",
    body: "Alex invited you to join ReUI Pro.",
    time: "1h ago",
    unread: false,
    actions: [
      { label: "Accept", variant: "default" },
      { label: "Decline", variant: "outline" },
    ],
  },
  {
    id: "n7",
    type: "deployment",
    variant: "success",
    title: "Deployment successful",
    body: "Production branch deployed to Vercel.",
    time: "2h ago",
    meta: { label: "Env", value: "Production", color: "text-success" },
  },
  {
    id: "n8",
    type: "achievement",
    variant: "success",
    title: "Monthly milestone reached!",
    body: "100 of 100 tasks completed this month.",
    time: "4h ago",
    progress: 60,
    progressVariant: "success",
    meta: { label: "Goal", value: "100 tasks", color: "text-success" },
  },
  {
    id: "n9",
    type: "security",
    variant: "destructive",
    title: "New sign-in detected",
    body: "We noticed a new login from Mac OS, Chrome.",
    time: "Yesterday",
  },
  {
    id: "n10",
    type: "billing",
    variant: "info",
    title: "Payment processed",
    body: "Your monthly subscription was renewed.",
    time: "2 days ago",
    badge: "$49.00",
  },
]
