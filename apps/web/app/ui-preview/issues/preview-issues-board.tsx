"use client";

import Link from "next/link";
import {
  BarChart3,
  Bot,
  ChevronDown,
  CircleUser,
  Columns3,
  Filter,
  FolderKanban,
  Inbox,
  Layers3,
  ListTodo,
  MessageSquare,
  Monitor,
  MoreHorizontal,
  Plus,
  Settings,
  SlidersHorizontal,
  SquarePen,
  Users,
  Zap,
} from "lucide-react";
import type { LucideIcon } from "lucide-react";
import type { IssuePriority, IssueStatusCategory } from "@patchbay/core/types";
import { Button } from "@patchbay/ui/components/ui/button";
import { cn } from "@patchbay/ui/lib/utils";
import { useT } from "@patchbay/views/i18n";
import { PriorityIcon, StatusIcon } from "@patchbay/views/issues/components";

type PreviewCard = {
  identifier: string;
  title: string;
  description: string;
  priority: IssuePriority;
  label: string;
  assignee: string;
};

type PreviewColumn = {
  status: IssueStatusCategory;
  className: string;
  cards: readonly PreviewCard[];
};

const PREVIEW_COLUMNS: readonly PreviewColumn[] = [
  {
    status: "backlog",
    className: "bg-surface",
    cards: [
      {
        identifier: "PRE-101",
        title: "Refine workspace onboarding",
        description: "Make the first-run path easier to understand.",
        priority: "high",
        label: "Product",
        assignee: "Alex",
      },
    ],
  },
  {
    status: "todo",
    className: "bg-surface",
    cards: [
      {
        identifier: "PRE-102",
        title: "Polish issue board empty states",
        description: "Keep the board useful before real work arrives.",
        priority: "medium",
        label: "UI",
        assignee: "Mika",
      },
      {
        identifier: "PRE-103",
        title: "Add keyboard shortcuts",
        description: "Expose the common actions without extra chrome.",
        priority: "low",
        label: "UX",
        assignee: "Alex",
      },
    ],
  },
  {
    status: "in_progress",
    className: "bg-amber-500/[0.04]",
    cards: [
      {
        identifier: "PRE-104",
        title: "Add real-time status indicator",
        description: "Show when an agent is actively working on an issue.",
        priority: "urgent",
        label: "Realtime",
        assignee: "Agent",
      },
    ],
  },
  {
    status: "in_review",
    className: "bg-emerald-500/[0.035]",
    cards: [
      {
        identifier: "PRE-105",
        title: "Check responsive sidebar",
        description: "Make the workspace navigation feel balanced at every width.",
        priority: "medium",
        label: "Review",
        assignee: "Alex",
      },
    ],
  },
  {
    status: "done",
    className: "bg-blue-500/[0.04]",
    cards: [
      {
        identifier: "PRE-106",
        title: "Split web and API dev commands",
        description: "Let visual work start without the full local stack.",
        priority: "none",
        label: "Developer experience",
        assignee: "Alex",
      },
    ],
  },
];

type NavItem = {
  icon: LucideIcon;
  label: string;
  active?: boolean;
};

function PreviewNavItem({ icon: Icon, label, active }: NavItem) {
  return (
    <button
      type="button"
      className={cn(
        "flex h-8 w-full items-center gap-2 rounded-md px-2 text-left text-body text-muted-foreground transition-colors hover:bg-sidebar-accent hover:text-sidebar-accent-foreground",
        active && "bg-sidebar-accent text-sidebar-accent-foreground",
      )}
      aria-current={active ? "page" : undefined}
    >
      <Icon className="size-4 shrink-0" />
      <span className="truncate">{label}</span>
    </button>
  );
}

function PreviewIssueCard({ card }: { card: PreviewCard }) {
  const { t } = useT("issues");

  return (
    <article className="group rounded-lg border-[0.5px] border-surface-border bg-background/80 px-2.5 py-3 shadow-[var(--surface-shadow)] transition-colors hover:border-foreground/15 hover:bg-surface-hover">
      <div className="flex items-center justify-between gap-2">
        <div className="flex min-w-0 items-center gap-1.5">
          <span
            className="inline-flex shrink-0"
            aria-label={t(($) => $.priority[card.priority])}
          >
            <PriorityIcon priority={card.priority} className="size-3.5" />
          </span>
          <span className="truncate text-caption text-muted-foreground">
            {card.identifier}
          </span>
        </div>
        <button
          type="button"
          className="inline-flex size-6 shrink-0 items-center justify-center rounded-md text-muted-foreground opacity-0 transition-opacity hover:bg-muted hover:text-foreground group-hover:opacity-100"
          aria-label={t(($) => $.display.tooltip)}
        >
          <MoreHorizontal className="size-3.5" />
        </button>
      </div>
      <h3 className="mt-1 text-body font-medium leading-snug">{card.title}</h3>
      <p className="mt-1 line-clamp-2 text-caption text-muted-foreground">
        {card.description}
      </p>
      <div className="mt-2 flex items-center justify-between gap-2">
        <span className="max-w-[9rem] truncate rounded-full bg-muted/60 px-1.5 py-0.5 text-micro text-muted-foreground">
          {card.label}
        </span>
        <span className="flex min-w-0 items-center gap-1 text-caption text-muted-foreground">
            <span className="inline-flex size-4 shrink-0 items-center justify-center rounded-full bg-brand/15 text-micro font-semibold text-brand">
            {card.assignee.slice(0, 1)}
          </span>
          <span className="truncate">{card.assignee}</span>
        </span>
      </div>
    </article>
  );
}

function PreviewBoardColumn({ column }: { column: PreviewColumn }) {
  const { t } = useT("issues");

  return (
    <section
      className={cn(
        "flex min-w-[16rem] flex-1 flex-col rounded-xl border border-border/60",
        column.className,
      )}
      aria-label={t(($) => $.status[column.status])}
    >
      <header className="flex h-11 shrink-0 items-center justify-between gap-2 px-3">
        <div className="flex min-w-0 items-center gap-2">
          <StatusIcon status={column.status} className="size-3.5 shrink-0" />
          <span className="truncate text-caption font-semibold">
            {t(($) => $.status[column.status])}
          </span>
          <span className="text-caption tabular-nums text-muted-foreground">
            {column.cards.length}
          </span>
        </div>
        <div className="flex items-center gap-0.5 text-muted-foreground">
          <button
            type="button"
            className="inline-flex size-6 items-center justify-center rounded-md hover:bg-muted hover:text-foreground"
            aria-label={t(($) => $.board.add_issue_tooltip)}
          >
            <Plus className="size-3.5" />
          </button>
          <button
            type="button"
            className="inline-flex size-6 items-center justify-center rounded-md hover:bg-muted hover:text-foreground"
            aria-label={t(($) => $.display.tooltip)}
          >
            <MoreHorizontal className="size-3.5" />
          </button>
        </div>
      </header>
      <div className="min-h-0 flex-1 space-y-2 overflow-y-auto px-2 pb-2">
        {column.cards.map((card) => (
          <PreviewIssueCard key={card.identifier} card={card} />
        ))}
      </div>
    </section>
  );
}

export function PreviewIssuesBoard() {
  const { t: layoutT } = useT("layout");
  const { t } = useT("issues");

  const personalNav: NavItem[] = [
    { icon: Inbox, label: layoutT(($) => $.nav.inbox) },
    { icon: MessageSquare, label: layoutT(($) => $.nav.chat) },
    { icon: CircleUser, label: layoutT(($) => $.nav.my_issues) },
  ];
  const workspaceNav: NavItem[] = [
    { icon: ListTodo, label: layoutT(($) => $.nav.issues), active: true },
    { icon: FolderKanban, label: layoutT(($) => $.nav.projects) },
    { icon: Zap, label: layoutT(($) => $.nav.autopilots) },
    { icon: Bot, label: layoutT(($) => $.nav.agents) },
    { icon: Users, label: layoutT(($) => $.nav.squads) },
    { icon: BarChart3, label: layoutT(($) => $.nav.usage) },
  ];
  const configureNav: NavItem[] = [
    { icon: Monitor, label: layoutT(($) => $.nav.runtimes) },
    { icon: Layers3, label: layoutT(($) => $.nav.skills) },
    { icon: Settings, label: layoutT(($) => $.nav.settings) },
  ];

  return (
    <div
      className="flex h-svh min-h-[640px] overflow-hidden bg-app-shell text-foreground"
      data-preview-no-backend="true"
    >
      <aside className="hidden w-60 shrink-0 flex-col border-r bg-sidebar md:flex">
        <div className="flex h-12 shrink-0 items-center justify-between px-4">
          <Link
            href="/ui-preview"
            className="flex min-w-0 items-center gap-2 text-body font-medium"
          >
            <span className="inline-flex size-6 shrink-0 items-center justify-center rounded-full bg-muted text-caption font-semibold">
              P
            </span>
            <span className="truncate">Preview</span>
            <ChevronDown className="size-3.5 shrink-0 text-muted-foreground" />
          </Link>
        </div>

        <div className="px-3 pb-3">
          <button
            type="button"
            className="flex h-8 w-full items-center gap-2 rounded-md px-2 text-body text-muted-foreground transition-colors hover:bg-sidebar-accent hover:text-sidebar-accent-foreground"
          >
            <SquarePen className="size-4" />
            <span className="flex-1 text-left">{layoutT(($) => $.sidebar.new_issue)}</span>
            <kbd className="rounded border border-border/70 px-1 text-micro text-muted-foreground">{layoutT(($) => $.sidebar.new_issue_shortcut)}</kbd>
          </button>
        </div>

        <nav className="min-h-0 flex-1 space-y-5 overflow-y-auto px-3" aria-label={layoutT(($) => $.sidebar.workspace_group)}>
          <div className="space-y-1">
            {personalNav.map((item) => (
              <PreviewNavItem key={item.label} {...item} />
            ))}
          </div>
          <div className="space-y-1">
            <p className="px-2 text-micro font-medium uppercase tracking-wide text-muted-foreground">
              {layoutT(($) => $.sidebar.workspace_group)}
            </p>
            {workspaceNav.map((item) => (
              <PreviewNavItem key={item.label} {...item} />
            ))}
          </div>
          <div className="space-y-1">
            <p className="px-2 text-micro font-medium uppercase tracking-wide text-muted-foreground">
              {layoutT(($) => $.sidebar.configure_group)}
            </p>
            {configureNav.map((item) => (
              <PreviewNavItem key={item.label} {...item} />
            ))}
          </div>
        </nav>

        <div className="flex items-center gap-2 border-t px-4 py-3 text-caption text-muted-foreground">
          <span className="inline-flex size-6 items-center justify-center rounded-full bg-muted font-semibold text-foreground">
            A
          </span>
          <span className="truncate">{layoutT(($) => $.sidebar.workspace_group)}</span>
        </div>
      </aside>

      <main className="flex min-w-0 flex-1 flex-col overflow-hidden bg-background">
        <header className="flex h-12 shrink-0 items-center gap-2 border-b px-4">
          <ListTodo className="size-4 text-muted-foreground" />
          <h1 className="text-body font-medium">{t(($) => $.page.breadcrumb_title)}</h1>
        </header>

        <div className="flex min-h-12 shrink-0 items-center justify-between gap-3 border-b px-4 py-2">
          <div className="flex min-w-0 items-center gap-1.5">
            <Button variant="brandSubtle" size="sm" aria-pressed="true">
              {t(($) => $.scope.all_label)}
            </Button>
            <Button variant="ghost" size="sm">
              {t(($) => $.scope.members_label)}
            </Button>
            <Button variant="ghost" size="sm">
              {t(($) => $.scope.agents_label)}
            </Button>
          </div>
          <div className="flex shrink-0 items-center gap-1.5">
            <Button variant="outline" size="sm">
              <CircleUser className="size-3.5" />
              {t(($) => $.scope.all_label)}
            </Button>
            <Button variant="outline" size="sm">
              <Filter className="size-3.5" />
              {t(($) => $.filters.tooltip)}
            </Button>
            <Button variant="outline" size="sm">
              <SlidersHorizontal className="size-3.5" />
              {t(($) => $.display.button)}
            </Button>
            <Button variant="outline" size="sm" aria-pressed="true">
              <Columns3 className="size-3.5" />
              {t(($) => $.view.board)}
            </Button>
          </div>
        </div>

        <div className="min-h-0 flex-1 overflow-x-auto p-3" data-preview-board="true">
          <div className="flex h-full min-w-[82rem] gap-3">
            {PREVIEW_COLUMNS.map((column) => (
              <PreviewBoardColumn key={column.status} column={column} />
            ))}
          </div>
        </div>
      </main>
    </div>
  );
}
