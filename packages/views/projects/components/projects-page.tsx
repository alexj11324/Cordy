"use client";

import { useCallback, useMemo, useState, type MouseEvent } from "react";
import {
  ArrowDown,
  ArrowUp,
  ChevronDown,
  ExternalLink,
  Filter,
  FolderKanban,
  LayoutGrid,
  MoreHorizontal,
  Pin,
  PinOff,
  Plus,
  Rows3,
  Search,
  Trash2,
  X,
} from "lucide-react";
import {
  useTable,
  type ColumnDef,
  type ColumnVisibilityState,
  type RowSelectionState,
  type SortingState,
} from "@tanstack/react-table";
import { useQuery } from "@tanstack/react-query";
import { toast } from "sonner";
import {
  projectListOptions,
  useUpdateProject,
  useDeleteProject,
  useProjectViewStore,
  type ProjectColumnKey,
  type ProjectListFilters,
  type ProjectSortField,
  type ProjectViewMode,
} from "@orvilo/core/projects";
import {
  pinListOptions,
  useCreatePin,
  useDeletePin,
} from "@orvilo/core/pins";
import { useWorkspaceId } from "@orvilo/core/hooks";
import { useWorkspacePaths } from "@orvilo/core/paths";
import { useAuthStore } from "@orvilo/core/auth";
import { useActorName } from "@orvilo/core/workspace/hooks";
import { memberListOptions } from "@orvilo/core/workspace/queries";
import { useModalStore } from "@orvilo/core/modals";
import { AppLink, useIntentNavigate, useRowLink } from "../../navigation";
import { ActorAvatar } from "../../common/actor-avatar";
import { FILTER_ITEM_CLASS, HoverCheck } from "../../common/hover-check";
import { Skeleton } from "@orvilo/ui/components/ui/skeleton";
import { Button } from "@orvilo/ui/components/ui/button";
import { Input } from "@orvilo/ui/components/ui/input";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@orvilo/ui/components/ui/dialog";
import {
  DropdownMenu,
  DropdownMenuCheckboxItem,
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
} from "@orvilo/ui/components/ui/dropdown-menu";
import {
  DataGrid,
  dataGridFeatures,
  type DataGridFeatures,
} from "@orvilo/ui/components/reui/data-grid/data-grid";
import { DataGridColumnHeader } from "@orvilo/ui/components/reui/data-grid/data-grid-column-header";
import { DataGridScrollArea } from "@orvilo/ui/components/reui/data-grid/data-grid-scroll-area";
import {
  DataGridTable,
  DataGridTableRowSelect,
  DataGridTableRowSelectAll,
} from "@orvilo/ui/components/reui/data-grid/data-grid-table";
import {
  Popover,
  PopoverContent,
  PopoverTrigger,
} from "@orvilo/ui/components/ui/popover";
import { Switch } from "@orvilo/ui/components/ui/switch";
import {
  Tooltip,
  TooltipContent,
  TooltipTrigger,
} from "@orvilo/ui/components/ui/tooltip";
import type {
  MemberWithUser,
  Project,
  ProjectPriority,
  ProjectStatus,
  UpdateProjectRequest,
} from "@orvilo/core/types";
import {
  CollectionPageHeaderAction,
  CollectionPageState,
} from "../../layout/collection-page";
import { ShellHeaderActions } from "../../layout/shell-header";
import { ProjectIcon } from "./project-icon";
import { useLocale, useT } from "../../i18n";
import { matchesPinyin } from "../../editor/extensions/pinyin-match";
import { useFormatRelativeDate } from "./labels";
import { ProjectStatusBadge, ProjectPriorityBadge } from "./project-badge";
import { ProjectLeadPicker } from "./project-lead-picker";
import { PAGE_GUTTER, PAGE_TOOLBAR } from "../../layout/page-header";
import { cn } from "@orvilo/ui/lib/utils";
import { formatDateOnly, isPastDateOnly } from "@orvilo/core/issues/date";

// Sort order maps for the enum columns (header sort needs a total order).
const PRIORITY_ORDER: Record<ProjectPriority, number> = {
  urgent: 4,
  high: 3,
  medium: 2,
  low: 1,
  none: 0,
};
const STATUS_ORDER: Record<ProjectStatus, number> = {
  planned: 0,
  in_progress: 1,
  paused: 2,
  completed: 3,
  cancelled: 4,
};

const progressOf = (p: Project) =>
  p.issue_count > 0 ? p.done_count / p.issue_count : -1;

// Composite "type:id" lead value so the string[] filter holds member/agent
// refs alike.
function leadFilterValue(p: Project): string | null {
  return p.lead_type && p.lead_id ? `${p.lead_type}:${p.lead_id}` : null;
}

const stopRowNavigation = (e: MouseEvent) => e.stopPropagation();

const PROJECT_GRID_COLUMN_IDS: Record<ProjectColumnKey, string> = {
  priority: "priority",
  progress: "health",
  lead: "lead",
  issues: "issues",
  created: "targetDate",
};

const PROJECT_GRID_SORT_IDS: Record<ProjectSortField, string> = {
  name: "name",
  priority: "priority",
  status: "status",
  progress: "health",
  created: "targetDate",
};

function projectSortFieldFromGridId(id: string): ProjectSortField | null {
  for (const [field, columnId] of Object.entries(PROJECT_GRID_SORT_IDS)) {
    if (columnId === id) return field as ProjectSortField;
  }
  return null;
}

type ProjectHealth = "on_track" | "at_risk" | "off_track" | "no_update";

const PROJECT_HEALTH_ORDER: Record<ProjectHealth, number> = {
  no_update: 0,
  off_track: 1,
  at_risk: 2,
  on_track: 3,
};

function projectHealthOf(project: Project): ProjectHealth {
  if (project.status === "completed") return "on_track";
  if (project.status === "cancelled") return "off_track";
  if (project.status === "paused") return "at_risk";
  if (project.issue_count === 0) return "no_update";
  if (project.due_date && isPastDateOnly(project.due_date)) return "off_track";

  const progress = progressOf(project);
  if (progress >= 0.66) return "on_track";
  if (progress > 0) return "at_risk";
  return "no_update";
}

function compareProjects(
  a: Project,
  b: Project,
  field: ProjectSortField,
  direction: "asc" | "desc",
): number {
  const dir = direction === "asc" ? 1 : -1;
  if (field === "name") return a.title.localeCompare(b.title) * dir;
  if (field === "priority") {
    return (
      (PRIORITY_ORDER[a.priority] - PRIORITY_ORDER[b.priority]) * dir ||
      a.title.localeCompare(b.title)
    );
  }
  if (field === "status") {
    return (
      (STATUS_ORDER[a.status] - STATUS_ORDER[b.status]) * dir ||
      a.title.localeCompare(b.title)
    );
  }
  if (field === "progress") {
    return (
      (PROJECT_HEALTH_ORDER[projectHealthOf(a)] - PROJECT_HEALTH_ORDER[projectHealthOf(b)]) * dir ||
      (progressOf(a) - progressOf(b)) * dir ||
      a.title.localeCompare(b.title)
    );
  }

  const aDate = a.due_date ? Date.parse(a.due_date) : Number.POSITIVE_INFINITY;
  const bDate = b.due_date ? Date.parse(b.due_date) : Number.POSITIVE_INFINITY;
  return (aDate - bDate) * dir || a.title.localeCompare(b.title);
}

// Compact rows own whole-row navigation; callers stop propagation around this
// menu so action clicks do not bubble into the rowLink handler.
function ProjectRowActions({
  project,
  pinned,
  canDelete,
}: {
  project: Project;
  pinned: boolean;
  canDelete: boolean;
}) {
  const { t } = useT("projects");
  const { t: tCommon } = useT("common");
  const wsPaths = useWorkspacePaths();
  const intentNavigate = useIntentNavigate();
  const createPin = useCreatePin();
  const deletePin = useDeletePin();
  const deleteProject = useDeleteProject();
  const [deleteOpen, setDeleteOpen] = useState(false);

  const togglePin = () => {
    if (pinned) deletePin.mutate({ itemType: "project", itemId: project.id });
    else createPin.mutate({ item_type: "project", item_id: project.id });
  };

  return (
    <>
      <DropdownMenu>
        <DropdownMenuTrigger
          render={
            <button
              type="button"
              aria-label={t(($) => $.page.row_menu)}
              className="flex size-7 items-center justify-center rounded-md text-muted-foreground opacity-0 transition-opacity hover:bg-accent hover:text-accent-foreground group-hover/row:opacity-100 data-popup-open:bg-accent data-popup-open:opacity-100 data-popup-open:text-accent-foreground"
            >
              <MoreHorizontal className="size-4" />
            </button>
          }
        />
        <DropdownMenuContent align="end" className="w-44">
          <DropdownMenuItem
            onClick={() =>
              intentNavigate(
                wsPaths.projectDetail(project.id),
                "foreground-tab",
                project.title,
              )
            }
          >
            <ExternalLink className="size-3.5" />
            {tCommon(($) => $.navigation.open_in_new_tab)}
          </DropdownMenuItem>
          <DropdownMenuSeparator />
          <DropdownMenuItem onClick={togglePin}>
            {pinned ? (
              <PinOff className="size-3.5" />
            ) : (
              <Pin className="size-3.5" />
            )}
            {pinned ? t(($) => $.page.unpin) : t(($) => $.page.pin)}
          </DropdownMenuItem>
          {canDelete && (
            <>
              <DropdownMenuSeparator />
              <DropdownMenuItem
                variant="destructive"
                onClick={() => setDeleteOpen(true)}
              >
                <Trash2 className="size-3.5" />
                {t(($) => $.page.delete)}
              </DropdownMenuItem>
            </>
          )}
        </DropdownMenuContent>
      </DropdownMenu>

      <Dialog open={deleteOpen} onOpenChange={setDeleteOpen}>
        <DialogContent className="sm:max-w-md">
          <DialogHeader>
            <DialogTitle>{t(($) => $.delete_dialog.title)}</DialogTitle>
            <DialogDescription>
              {t(($) => $.delete_dialog.description)}
            </DialogDescription>
          </DialogHeader>
          <DialogFooter>
            <Button
              type="button"
              variant="outline"
              size="sm"
              onClick={() => setDeleteOpen(false)}
            >
              {t(($) => $.delete_dialog.cancel)}
            </Button>
            <Button
              type="button"
              variant="destructive"
              size="sm"
              onClick={() => {
                deleteProject.mutate(project.id, {
                  onError: (err) =>
                    toast.error(
                      err instanceof Error ? err.message : String(err),
                    ),
                });
                setDeleteOpen(false);
              }}
            >
              {t(($) => $.delete_dialog.confirm)}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </>
  );
}

function ProjectNameCell({ project }: { project: Project }) {
  return (
    <div className="flex min-w-0 items-center gap-2">
      <ProjectIcon project={project} size="sm" />
      <span className="min-w-0 truncate text-body font-medium">{project.title}</span>
    </div>
  );
}

function ProjectHealthCell({ project }: { project: Project }) {
  const { t } = useT("projects");
  const health = projectHealthOf(project);
  const progress = project.issue_count > 0
    ? Math.round((project.done_count / project.issue_count) * 100)
    : null;
  const dotClass = {
    on_track: "bg-success",
    at_risk: "bg-warning",
    off_track: "bg-destructive",
    no_update: "bg-muted-foreground/50",
  }[health];

  return (
    <div className="flex min-w-0 items-center gap-2">
      <span className={cn("size-2 shrink-0 rounded-full", dotClass)} aria-hidden="true" />
      <span className="min-w-0 truncate text-body">
        {t(($) => $.health[health])}
      </span>
      {progress !== null && (
        <span className="ml-auto shrink-0 text-caption tabular-nums text-muted-foreground">
          {progress}%
        </span>
      )}
    </div>
  );
}

function ProjectStatusCell({ project }: { project: Project }) {
  const updateProject = useUpdateProject();
  const handleUpdate = useCallback(
    (data: UpdateProjectRequest) => updateProject.mutate({ id: project.id, ...data }),
    [project.id, updateProject],
  );
  return (
    <div onClick={stopRowNavigation} onAuxClick={stopRowNavigation}>
      <ProjectStatusBadge project={project} handleUpdate={handleUpdate} align="start" />
    </div>
  );
}

function ProjectPriorityCell({ project }: { project: Project }) {
  const updateProject = useUpdateProject();
  const handleUpdate = useCallback(
    (data: UpdateProjectRequest) => updateProject.mutate({ id: project.id, ...data }),
    [project.id, updateProject],
  );
  return (
    <div onClick={stopRowNavigation} onAuxClick={stopRowNavigation}>
      <ProjectPriorityBadge project={project} handleUpdate={handleUpdate} align="start" />
    </div>
  );
}

function ProjectLeadCell({ project }: { project: Project }) {
  const updateProject = useUpdateProject();
  const handleUpdate = useCallback(
    (data: UpdateProjectRequest) => updateProject.mutate({ id: project.id, ...data }),
    [project.id, updateProject],
  );
  return (
    <div onClick={stopRowNavigation} onAuxClick={stopRowNavigation}>
      <ProjectLeadPicker
        project={project}
        handleUpdate={handleUpdate}
        align="start"
        renderTrigger={(leadName) => (
          <button
            type="button"
            className="flex min-w-0 items-center gap-1.5 rounded px-1 py-0.5 transition-colors hover:bg-accent/60"
          >
            {project.lead_type && project.lead_id ? (
              <ActorAvatar actorType={project.lead_type} actorId={project.lead_id} size="sm" enableHoverCard />
            ) : (
              <span className="inline-flex size-[18px] rounded-full border border-dashed border-muted-foreground/30" />
            )}
            <span className="min-w-0 truncate text-body text-muted-foreground">
              {leadName ?? "—"}
            </span>
          </button>
        )}
      />
    </div>
  );
}

function ProjectTargetDateCell({ project, locale }: { project: Project; locale: string }) {
  const label = formatDateOnly(
    project.due_date,
    { year: "numeric", month: "short", day: "numeric" },
    locale,
  );
  const overdue = project.status !== "completed" && isPastDateOnly(project.due_date);
  return (
    <span
      className={cn(
        "block truncate text-body tabular-nums text-muted-foreground",
        overdue && "text-destructive",
      )}
    >
      {label || "—"}
    </span>
  );
}

function ProjectActionsCell({
  project,
  pinned,
  canDelete,
}: {
  project: Project;
  pinned: boolean;
  canDelete: boolean;
}) {
  return (
    <div onClick={stopRowNavigation} onAuxClick={stopRowNavigation} className="flex justify-end">
      <ProjectRowActions project={project} pinned={pinned} canDelete={canDelete} />
    </div>
  );
}

// ---------------------------------------------------------------------------
// Card (comfortable) view — kept from the prior page.
// ---------------------------------------------------------------------------

function ProjectCard({
  project,
  pinned,
  canDelete,
}: {
  project: Project;
  pinned: boolean;
  canDelete: boolean;
}) {
  const { t } = useT("projects");
  const wsPaths = useWorkspacePaths();
  const formatRelativeDate = useFormatRelativeDate();
  const updateProject = useUpdateProject();
  const handleUpdate = useCallback(
    (data: UpdateProjectRequest) => updateProject.mutate({ id: project.id, ...data }),
    [project.id, updateProject],
  );
  const progressPercent =
    project.issue_count > 0
      ? Math.round((project.done_count / project.issue_count) * 100)
      : 0;

  return (
    <div className="group/card group/row flex flex-col rounded-md border bg-card transition-colors hover:border-primary/50">
      <div className="p-3 pb-2">
        <div className="flex items-center gap-2">
          <AppLink
            href={wsPaths.projectDetail(project.id)}
            className="flex min-w-0 flex-1 items-center gap-2"
          >
            <ProjectIcon project={project} size="sm" />
            <h3 className="truncate text-body font-medium">{project.title}</h3>
          </AppLink>
          <ProjectRowActions project={project} pinned={pinned} canDelete={canDelete} />
          <ProjectStatusBadge project={project} handleUpdate={handleUpdate} triggerClassName="shrink-0" />
        </div>

        {project.issue_count > 0 ? (
          <div className="flex items-center justify-end gap-1.5 pt-2">
            <div className="relative h-4 w-4">
              <svg className="h-4 w-4 -rotate-90" viewBox="0 0 16 16">
                <circle className="text-muted" strokeWidth="2" stroke="currentColor" fill="none" r="6" cx="8" cy="8" />
                <circle
                  className="text-emerald-500"
                  strokeWidth="2"
                  stroke="currentColor"
                  fill="none"
                  r="6"
                  cx="8"
                  cy="8"
                  strokeDasharray={`${progressPercent * 0.377} 37.7`}
                  strokeLinecap="round"
                />
              </svg>
            </div>
            <span className="text-micro tabular-nums text-muted-foreground">
              {project.done_count}/{project.issue_count}
            </span>
          </div>
        ) : (
          <span className="flex justify-end pt-2 text-micro text-muted-foreground">
            {t(($) => $.detail.no_issues_yet)}
          </span>
        )}
      </div>

      <div className="mt-0 flex items-center justify-between border-t px-3 pb-3 pt-2">
        <ProjectLeadPicker
          project={project}
          handleUpdate={handleUpdate}
          renderTrigger={(leadName) => (
            <button type="button" className="-mx-1.5 flex items-center gap-1.5 rounded px-1.5 py-0.5 transition-colors hover:bg-accent/60">
              {project.lead_type && project.lead_id ? (
                <ActorAvatar actorType={project.lead_type} actorId={project.lead_id} size="sm" enableHoverCard />
              ) : (
                <span className="inline-flex h-5 w-5 rounded-full border border-dashed border-muted-foreground/30" />
              )}
              <span className="max-w-[60px] truncate text-micro text-muted-foreground">
                {leadName ?? t(($) => $.lead.no_lead)}
              </span>
            </button>
          )}
        />
        <div className="flex items-center gap-2">
          <ProjectPriorityBadge project={project} handleUpdate={handleUpdate} align="start" />
          <span className="text-micro text-muted-foreground">
            {formatRelativeDate(project.created_at)}
          </span>
        </div>
      </div>
    </div>
  );
}

// ---------------------------------------------------------------------------
// Toolbar — search + result count + filter + display (compact only) + view
// toggle.
// ---------------------------------------------------------------------------

const STATUS_VALUES: ProjectStatus[] = [
  "planned",
  "in_progress",
  "paused",
  "completed",
  "cancelled",
];
const PRIORITY_VALUES: ProjectPriority[] = ["urgent", "high", "medium", "low", "none"];
const COLUMN_KEYS: ProjectColumnKey[] = ["priority", "progress", "lead", "issues", "created"];
const SORT_FIELDS: ProjectSortField[] = ["name", "priority", "status", "progress", "created"];

function countActiveFilters(f: ProjectListFilters): number {
  let c = 0;
  if (f.statuses.length) c++;
  if (f.priorities.length) c++;
  if (f.leads.length) c++;
  return c;
}

// Batch toolbar — page-anchored (not viewport). Pin all selected (any
// member) + Delete (workspace admin). Mirrors the other lists.
function ProjectBatchToolbar({
  rows,
  pinnedIds,
  canDelete,
  onClear,
}: {
  rows: Project[];
  pinnedIds: Set<string>;
  canDelete: boolean;
  onClear: () => void;
}) {
  const { t } = useT("projects");
  const createPin = useCreatePin();
  const deleteProject = useDeleteProject();
  const [confirmDelete, setConfirmDelete] = useState(false);

  if (rows.length === 0) return null;
  const anyUnpinned = rows.some((p) => !pinnedIds.has(p.id));

  return (
    <>
      <div className="absolute bottom-6 left-1/2 z-50 flex -translate-x-1/2 items-center gap-1 rounded-lg border bg-background px-2 py-1.5 shadow-lg max-md:above-chat-launcher">
        <div className="mr-1 flex items-center gap-1.5 border-r pl-1 pr-2">
          <span className="text-body font-medium">
            {t(($) => $.page.selected, { count: rows.length })}
          </span>
          <button
            type="button"
            aria-label={t(($) => $.page.clear_selection)}
            onClick={onClear}
            className="rounded p-0.5 transition-colors hover:bg-accent"
          >
            <X className="size-3.5 text-muted-foreground" />
          </button>
        </div>
        {anyUnpinned && (
          <Button
            variant="ghost"
            size="sm"
            onClick={() => {
              for (const p of rows) {
                if (!pinnedIds.has(p.id)) {
                  createPin.mutate({ item_type: "project", item_id: p.id });
                }
              }
              onClear();
            }}
          >
            <Pin className="mr-1 size-3.5" />
            {t(($) => $.page.pin)}
          </Button>
        )}
        {canDelete && (
          <Button
            variant="ghost"
            size="sm"
            className="text-destructive hover:text-destructive"
            onClick={() => setConfirmDelete(true)}
          >
            <Trash2 className="mr-1 size-3.5" />
            {t(($) => $.page.delete)}
          </Button>
        )}
      </div>

      <Dialog open={confirmDelete} onOpenChange={setConfirmDelete}>
        <DialogContent className="sm:max-w-md">
          <DialogHeader>
            <DialogTitle>{t(($) => $.delete_dialog.title)}</DialogTitle>
            <DialogDescription>{t(($) => $.delete_dialog.description)}</DialogDescription>
          </DialogHeader>
          <DialogFooter>
            <Button type="button" variant="outline" size="sm" onClick={() => setConfirmDelete(false)}>
              {t(($) => $.delete_dialog.cancel)}
            </Button>
            <Button
              type="button"
              variant="destructive"
              size="sm"
              onClick={() => {
                for (const p of rows) deleteProject.mutate(p.id);
                setConfirmDelete(false);
                onClear();
              }}
            >
              {t(($) => $.delete_dialog.confirm)}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </>
  );
}

// ---------------------------------------------------------------------------
// Page
// ---------------------------------------------------------------------------

export function ProjectsPage() {
  const { t } = useT("projects");
  const locale = useLocale();
  const wsId = useWorkspaceId();
  const wsPaths = useWorkspacePaths();
  const rowLink = useRowLink();
  const currentUser = useAuthStore((s) => s.user);
  const { getActorName } = useActorName();

  const viewMode = useProjectViewStore((s) => s.viewMode);
  const setViewMode = useProjectViewStore((s) => s.setViewMode);
  const sortField = useProjectViewStore((s) => s.sortField);
  const sortDirection = useProjectViewStore((s) => s.sortDirection);
  const hiddenColumns = useProjectViewStore((s) => s.hiddenColumns);
  const filters = useProjectViewStore((s) => s.filters);
  const setSortField = useProjectViewStore((s) => s.setSortField);
  const setSortDirection = useProjectViewStore((s) => s.setSortDirection);
  const toggleColumn = useProjectViewStore((s) => s.toggleColumn);
  const toggleFilter = useProjectViewStore((s) => s.toggleFilter);
  const clearFilters = useProjectViewStore((s) => s.clearFilters);
  const isCompact = viewMode === "compact";

  const { data: projects = [], isLoading } = useQuery(projectListOptions(wsId));
  const { data: members = [] } = useQuery(memberListOptions(wsId));
  const { data: pins = [] } = useQuery({
    ...pinListOptions(wsId, currentUser?.id ?? ""),
    enabled: !!wsId && !!currentUser?.id,
  });
  const openCreateProject = () => useModalStore.getState().open("create-project");

  const isWorkspaceAdmin = useMemo(() => {
    if (!currentUser) return false;
    const me = members.find((m: MemberWithUser) => m.user_id === currentUser.id);
    return me?.role === "owner" || me?.role === "admin";
  }, [members, currentUser]);

  const pinnedProjectIds = useMemo(() => {
    const s = new Set<string>();
    for (const pin of pins) if (pin.item_type === "project") s.add(pin.item_id);
    return s;
  }, [pins]);

  const [search, setSearch] = useState("");
  const [selectedIds, setSelectedIds] = useState<ReadonlySet<string>>(new Set());

  const activeFilterCount = countActiveFilters(filters);
  const hasActiveFilters = activeFilterCount > 0;

  // Filter option counts derive from the full set so toggling one dimension
  // doesn't make the others vanish.
  const leadOptions = useMemo(() => {
    const m = new Map<string, { type: string; id: string; count: number }>();
    for (const p of projects) {
      const v = leadFilterValue(p);
      if (!v || !p.lead_type || !p.lead_id) continue;
      const e = m.get(v);
      if (e) e.count += 1;
      else m.set(v, { type: p.lead_type, id: p.lead_id, count: 1 });
    }
    return m;
  }, [projects]);

  const filteredProjects = useMemo(() => {
    const q = search.trim().toLowerCase();
    return projects.filter((p) => {
      if (q && !p.title.toLowerCase().includes(q) && !matchesPinyin(p.title, q)) {
        return false;
      }
      if (filters.statuses.length && !filters.statuses.includes(p.status)) return false;
      if (filters.priorities.length && !filters.priorities.includes(p.priority)) {
        return false;
      }
      if (filters.leads.length) {
        const v = leadFilterValue(p);
        if (!v || !filters.leads.includes(v)) return false;
      }
      return true;
    });
  }, [projects, search, filters]);

  const visible = useMemo(
    () => [...filteredProjects].sort((a, b) => compareProjects(a, b, sortField, sortDirection)),
    [filteredProjects, sortField, sortDirection],
  );

  const selectedProjects = visible.filter((p) => selectedIds.has(p.id));

  const rowSelection = useMemo<RowSelectionState>(() => {
    const selection: RowSelectionState = {};
    for (const id of selectedIds) selection[id] = true;
    return selection;
  }, [selectedIds]);

  const columnVisibility = useMemo<ColumnVisibilityState>(
    () => Object.fromEntries(hiddenColumns.map((key) => [PROJECT_GRID_COLUMN_IDS[key], false])),
    [hiddenColumns],
  );

  const sorting = useMemo<SortingState>(
    () => [{ id: PROJECT_GRID_SORT_IDS[sortField], desc: sortDirection === "desc" }],
    [sortField, sortDirection],
  );

  const projectById = useMemo(
    () => new Map(projects.map((project) => [project.id, project])),
    [projects],
  );

  const columns = useMemo<ColumnDef<DataGridFeatures, Project>[]>(
    () => [
      {
        id: "select",
        header: () => <DataGridTableRowSelectAll />,
        cell: ({ row }) => <DataGridTableRowSelect row={row} />,
        size: 44,
        enableSorting: false,
        enableHiding: false,
        enableResizing: false,
        meta: { headerClassName: "ps-4!", cellClassName: "ps-4!" },
      },
      {
        accessorKey: "title",
        id: "name",
        header: ({ column }) => <DataGridColumnHeader column={column} />,
        cell: ({ row }) => <ProjectNameCell project={row.original} />,
        size: 280,
        minSize: 220,
        enableSorting: true,
        enableHiding: false,
        enableResizing: false,
        meta: { headerTitle: t(($) => $.table.name) },
      },
      {
        id: "health",
        accessorFn: (row) => progressOf(row),
        header: ({ column }) => <DataGridColumnHeader column={column} visibility />,
        cell: ({ row }) => <ProjectHealthCell project={row.original} />,
        sortFn: (rowA, rowB) =>
          PROJECT_HEALTH_ORDER[projectHealthOf(rowA.original)] -
            PROJECT_HEALTH_ORDER[projectHealthOf(rowB.original)] ||
          progressOf(rowA.original) - progressOf(rowB.original),
        size: 150,
        enableSorting: true,
        enableHiding: true,
        enableResizing: false,
        meta: { headerTitle: t(($) => $.table.health) },
      },
      {
        accessorKey: "priority",
        id: "priority",
        header: ({ column }) => <DataGridColumnHeader column={column} visibility />,
        cell: ({ row }) => <ProjectPriorityCell project={row.original} />,
        sortFn: (rowA, rowB) => PRIORITY_ORDER[rowA.original.priority] - PRIORITY_ORDER[rowB.original.priority],
        size: 140,
        enableSorting: true,
        enableHiding: true,
        enableResizing: false,
        meta: { headerTitle: t(($) => $.table.priority) },
      },
      {
        id: "lead",
        accessorFn: (row) => row.lead_id ?? "",
        header: ({ column }) => <DataGridColumnHeader column={column} visibility />,
        cell: ({ row }) => <ProjectLeadCell project={row.original} />,
        size: 190,
        enableSorting: false,
        enableHiding: true,
        enableResizing: false,
        meta: { headerTitle: t(($) => $.table.lead) },
      },
      {
        id: "targetDate",
        accessorFn: (row) => row.due_date ? Date.parse(row.due_date) : Number.POSITIVE_INFINITY,
        header: ({ column }) => <DataGridColumnHeader column={column} visibility />,
        cell: ({ row }) => <ProjectTargetDateCell project={row.original} locale={locale} />,
        sortFn: (rowA, rowB) => {
          const aDate = rowA.original.due_date ? Date.parse(rowA.original.due_date) : Number.POSITIVE_INFINITY;
          const bDate = rowB.original.due_date ? Date.parse(rowB.original.due_date) : Number.POSITIVE_INFINITY;
          return aDate - bDate;
        },
        size: 150,
        enableSorting: true,
        enableHiding: true,
        enableResizing: false,
        meta: { headerTitle: t(($) => $.table.target_date) },
      },
      {
        accessorKey: "issue_count",
        id: "issues",
        header: ({ column }) => <DataGridColumnHeader column={column} visibility />,
        cell: ({ row }) => (
          <span className="block text-right font-mono text-body tabular-nums text-muted-foreground">
            {row.original.issue_count}
          </span>
        ),
        size: 100,
        enableSorting: false,
        enableHiding: true,
        enableResizing: false,
        meta: { headerTitle: t(($) => $.table.issues) },
      },
      {
        accessorKey: "status",
        id: "status",
        header: ({ column }) => <DataGridColumnHeader column={column} />,
        cell: ({ row }) => <ProjectStatusCell project={row.original} />,
        sortFn: (rowA, rowB) => STATUS_ORDER[rowA.original.status] - STATUS_ORDER[rowB.original.status],
        size: 140,
        enableSorting: true,
        enableHiding: false,
        enableResizing: false,
        meta: { headerTitle: t(($) => $.table.status) },
      },
      {
        id: "actions",
        header: "",
        cell: ({ row }) => (
          <ProjectActionsCell
            project={row.original}
            pinned={pinnedProjectIds.has(row.original.id)}
            canDelete={isWorkspaceAdmin}
          />
        ),
        size: 52,
        enableSorting: false,
        enableHiding: false,
        enableResizing: false,
      },
    ],
    [isWorkspaceAdmin, locale, pinnedProjectIds, t],
  );

  const handleRowSelectionChange = useCallback(
    (updater: RowSelectionState | ((old: RowSelectionState) => RowSelectionState)) => {
      const next = typeof updater === "function" ? updater(rowSelection) : updater;
      setSelectedIds(new Set(Object.keys(next).filter((id) => next[id])));
    },
    [rowSelection],
  );

  const handleColumnVisibilityChange = useCallback(
    (updater: ColumnVisibilityState | ((old: ColumnVisibilityState) => ColumnVisibilityState)) => {
      const next = typeof updater === "function" ? updater(columnVisibility) : updater;
      for (const key of COLUMN_KEYS) {
        const columnId = PROJECT_GRID_COLUMN_IDS[key];
        const wasVisible = columnVisibility[columnId] !== false;
        const isVisible = next[columnId] !== false;
        if (wasVisible !== isVisible) toggleColumn(key);
      }
    },
    [columnVisibility, toggleColumn],
  );

  const handleGridSortingChange = useCallback(
    (updater: SortingState | ((old: SortingState) => SortingState)) => {
      const next = typeof updater === "function" ? updater(sorting) : updater;
      const first = next[0];
      if (!first) {
        setSortDirection("asc");
        return;
      }
      const nextField = projectSortFieldFromGridId(first.id);
      if (!nextField) return;
      if (nextField !== sortField) setSortField(nextField);
      setSortDirection(first.desc ? "desc" : "asc");
    },
    [setSortDirection, setSortField, sortField, sorting],
  );

  const handleGridMouseEvent = useCallback(
    (event: MouseEvent, kind: "click" | "auxclick") => {
      const target = event.target;
      if (!(target instanceof Element)) return;
      if (target.closest("button, a, input, select, textarea, [role=checkbox], [data-slot=checkbox]")) {
        return;
      }
      const row = target.closest<HTMLElement>("[data-row-id]");
      if (!row) return;
      const project = projectById.get(row.dataset.rowId ?? "");
      if (!project) return;
      const handlers = rowLink(wsPaths.projectDetail(project.id), project.title);
      if (kind === "auxclick") handlers.onAuxClick(event);
      else handlers.onClick(event);
    },
    [projectById, rowLink, wsPaths],
  );

  const handleGridMouseOver = useCallback(
    (event: MouseEvent) => {
      const target = event.target;
      if (!(target instanceof Element)) return;
      const row = target.closest<HTMLElement>("[data-row-id]");
      if (!row) return;
      if (event.relatedTarget instanceof Node && row.contains(event.relatedTarget)) return;
      const project = projectById.get(row.dataset.rowId ?? "");
      if (project) rowLink(wsPaths.projectDetail(project.id), project.title).onMouseEnter();
    },
    [projectById, rowLink, wsPaths],
  );

  const table = useTable({
    features: dataGridFeatures,
    data: filteredProjects,
    columns,
    getRowId: (row) => row.id,
    state: { columnVisibility, rowSelection, sorting },
    enableRowSelection: true,
    onColumnVisibilityChange: handleColumnVisibilityChange,
    onRowSelectionChange: handleRowSelectionChange,
    onSortingChange: handleGridSortingChange,
  });

  const sortLabel = (f: ProjectSortField) =>
    f === "name"
      ? t(($) => $.table.name)
      : f === "priority"
        ? t(($) => $.table.priority)
        : f === "status"
          ? t(($) => $.table.status)
          : f === "progress"
            ? t(($) => $.table.health)
            : t(($) => $.table.target_date);
  const columnLabel = (k: ProjectColumnKey) =>
    k === "priority"
      ? t(($) => $.table.priority)
      : k === "progress"
        ? t(($) => $.table.health)
        : k === "lead"
          ? t(($) => $.table.lead)
          : k === "issues"
            ? t(($) => $.table.issues)
            : t(($) => $.table.target_date);

  const showEmpty = !isLoading && projects.length === 0;
  const countBadge = (n: number) => (
    <span className="ml-auto pl-3 text-caption text-muted-foreground">{n}</span>
  );

  return (
    // relative: positioning anchor for the page-centered batch toolbar.
    <div className="relative flex flex-1 min-h-0 flex-col">
      <ShellHeaderActions>
        <CollectionPageHeaderAction
          icon={Plus}
          label={t(($) => $.page.new_project)}
          onClick={openCreateProject}
        />
      </ShellHeaderActions>

      {showEmpty ? (
        <CollectionPageState
          icon={FolderKanban}
          title={t(($) => $.page.empty)}
          actions={
            <Button size="sm" variant="outline" onClick={openCreateProject}>
              {t(($) => $.page.create_first)}
            </Button>
          }
        />
      ) : (
        <>
          {/* Toolbar */}
          <div className={PAGE_TOOLBAR}>
            <div className="flex min-w-0 items-center gap-2">
              <div className="relative hidden md:block">
                <Search className="pointer-events-none absolute left-2.5 top-1/2 h-3.5 w-3.5 -translate-y-1/2 text-muted-foreground" />
                <Input
                  value={search}
                  onChange={(e) => setSearch(e.target.value)}
                  aria-label={t(($) => $.page.search_placeholder)}
                  placeholder={t(($) => $.page.search_placeholder)}
                  className="h-8 w-56 pl-8 text-body"
                />
              </div>
              {(hasActiveFilters || search.trim().length > 0) && (
                <span
                  title={t(($) => $.toolbar.result_count_title)}
                  className="hidden shrink-0 text-caption tabular-nums text-muted-foreground md:inline"
                >
                  {visible.length} / {projects.length}
                </span>
              )}
            </div>

            <div className="flex shrink-0 items-center gap-1">
              {/* Filter */}
              <DropdownMenu>
                <DropdownMenuTrigger
                  render={
                    <Button
                      variant={hasActiveFilters ? "default" : "outline"}
                      size="sm"
                      className={
                        hasActiveFilters
                          ? "h-8 w-8 gap-1 bg-brand px-0 text-white hover:bg-brand/90 md:w-auto md:px-2.5"
                          : "h-8 w-8 gap-1 px-0 text-muted-foreground md:w-auto md:px-2.5"
                      }
                    >
                      <Filter className="size-3.5" />
                      {hasActiveFilters ? (
                        <>
                          <span className="hidden md:inline">
                            {t(($) => $.toolbar.filter_active_count, { count: activeFilterCount })}
                          </span>
                          <span className="tabular-nums md:hidden">{activeFilterCount}</span>
                        </>
                      ) : (
                        <span className="hidden md:inline">{t(($) => $.toolbar.filter_label)}</span>
                      )}
                      {hasActiveFilters && (
                        <span
                          role="button"
                          tabIndex={-1}
                          aria-label={t(($) => $.toolbar.clear_filters)}
                          className="-mr-1 ml-0.5 hidden rounded-sm p-0.5 hover:bg-white/20 md:inline-flex"
                          onClick={(e) => {
                            e.preventDefault();
                            e.stopPropagation();
                            clearFilters();
                          }}
                          onPointerDown={(e) => e.stopPropagation()}
                        >
                          <X className="size-3" />
                        </span>
                      )}
                    </Button>
                  }
                />
                <DropdownMenuContent align="end" className="w-auto">
                  <DropdownMenuSub>
                    <DropdownMenuSubTrigger>
                      <span className="flex-1">{t(($) => $.toolbar.section_status)}</span>
                      {filters.statuses.length > 0 && (
                        <span className="text-caption font-medium text-primary">{filters.statuses.length}</span>
                      )}
                    </DropdownMenuSubTrigger>
                    <DropdownMenuSubContent className="w-auto min-w-44">
                      {STATUS_VALUES.map((s) => (
                        <DropdownMenuCheckboxItem
                          key={s}
                          checked={filters.statuses.includes(s)}
                          onCheckedChange={() => toggleFilter("statuses", s)}
                          className={FILTER_ITEM_CLASS}
                        >
                          <HoverCheck checked={filters.statuses.includes(s)} />
                          {t(($) => $.status[s])}
                        </DropdownMenuCheckboxItem>
                      ))}
                    </DropdownMenuSubContent>
                  </DropdownMenuSub>
                  <DropdownMenuSub>
                    <DropdownMenuSubTrigger>
                      <span className="flex-1">{t(($) => $.toolbar.section_priority)}</span>
                      {filters.priorities.length > 0 && (
                        <span className="text-caption font-medium text-primary">{filters.priorities.length}</span>
                      )}
                    </DropdownMenuSubTrigger>
                    <DropdownMenuSubContent className="w-auto min-w-44">
                      {PRIORITY_VALUES.map((pr) => (
                        <DropdownMenuCheckboxItem
                          key={pr}
                          checked={filters.priorities.includes(pr)}
                          onCheckedChange={() => toggleFilter("priorities", pr)}
                          className={FILTER_ITEM_CLASS}
                        >
                          <HoverCheck checked={filters.priorities.includes(pr)} />
                          {t(($) => $.priority[pr])}
                        </DropdownMenuCheckboxItem>
                      ))}
                    </DropdownMenuSubContent>
                  </DropdownMenuSub>
                  <DropdownMenuSub>
                    <DropdownMenuSubTrigger>
                      <span className="flex-1">{t(($) => $.toolbar.section_lead)}</span>
                      {filters.leads.length > 0 && (
                        <span className="text-caption font-medium text-primary">{filters.leads.length}</span>
                      )}
                    </DropdownMenuSubTrigger>
                    <DropdownMenuSubContent className="max-h-72 w-auto min-w-48 overflow-y-auto">
                      {[...leadOptions.entries()].map(([value, { type, id, count }]) => (
                        <DropdownMenuCheckboxItem
                          key={value}
                          checked={filters.leads.includes(value)}
                          onCheckedChange={() => toggleFilter("leads", value)}
                          className={FILTER_ITEM_CLASS}
                        >
                          <HoverCheck checked={filters.leads.includes(value)} />
                          <ActorAvatar actorType={type} actorId={id} size="sm" />
                          <span className="min-w-0 truncate">{getActorName(type, id)}</span>
                          {countBadge(count)}
                        </DropdownMenuCheckboxItem>
                      ))}
                    </DropdownMenuSubContent>
                  </DropdownMenuSub>
                </DropdownMenuContent>
              </DropdownMenu>

              {/* Display (sort + columns). Always present — view mode is a
                  pure presentation choice and must not reshape the toolbar.
                  Sort applies to both views; the columns section is shown
                  only in the table view (cards have no columns). */}
              <Popover>
                  <Tooltip>
                    <PopoverTrigger
                      render={
                        <TooltipTrigger
                          render={
                            <Button variant="outline" size="sm" className="h-8 w-8 gap-1 px-0 text-muted-foreground md:w-auto md:px-2.5">
                              {sortDirection === "asc" ? <ArrowUp className="size-3.5" /> : <ArrowDown className="size-3.5" />}
                              <span className="hidden md:inline">{sortLabel(sortField)}</span>
                            </Button>
                          }
                        />
                      }
                    />
                    <TooltipContent side="bottom">{t(($) => $.toolbar.display)}</TooltipContent>
                  </Tooltip>
                  <PopoverContent align="end" className="w-64 p-0">
                    <div className="border-b px-3 py-2.5">
                      <span className="text-caption font-medium text-muted-foreground">{t(($) => $.toolbar.sort_by)}</span>
                      <div className="mt-2 flex items-center gap-1.5">
                        <DropdownMenu>
                          <DropdownMenuTrigger
                            render={
                              <Button variant="outline" size="sm" className="flex-1 justify-between text-caption">
                                {sortLabel(sortField)}
                                <ChevronDown className="size-3 text-muted-foreground" />
                              </Button>
                            }
                          />
                          <DropdownMenuContent align="start" className="w-auto">
                            <DropdownMenuRadioGroup
                              value={sortField}
                              onValueChange={(v) => setSortField(v as ProjectSortField)}
                            >
                              {SORT_FIELDS.map((f) => (
                                <DropdownMenuRadioItem key={f} value={f}>
                                  {sortLabel(f)}
                                </DropdownMenuRadioItem>
                              ))}
                            </DropdownMenuRadioGroup>
                          </DropdownMenuContent>
                        </DropdownMenu>
                        <Button
                          variant="outline"
                          size="icon-sm"
                          onClick={() => setSortDirection(sortDirection === "asc" ? "desc" : "asc")}
                          title={sortDirection === "asc" ? t(($) => $.toolbar.direction_asc) : t(($) => $.toolbar.direction_desc)}
                        >
                          {sortDirection === "asc" ? <ArrowUp className="size-3.5" /> : <ArrowDown className="size-3.5" />}
                        </Button>
                      </div>
                    </div>
                    {isCompact && (
                      <div className="px-3 py-2.5">
                        <span className="text-caption font-medium text-muted-foreground">{t(($) => $.toolbar.section_columns)}</span>
                        <div className="mt-2 space-y-2">
                          {COLUMN_KEYS.map((key) => (
                            <label key={key} className="flex cursor-pointer items-center justify-between">
                              <span className="text-body">{columnLabel(key)}</span>
                              <Switch size="sm" checked={!hiddenColumns.includes(key)} onCheckedChange={() => toggleColumn(key)} />
                            </label>
                          ))}
                        </div>
                      </div>
                    )}
                  </PopoverContent>
                </Popover>

              {/* View selector — a dropdown menu to pick the list view,
                  aligned with the issue list's view menu. The trigger shows
                  the active view; the menu carries every mode so new views
                  can be added as menu items. Pure presentation. */}
              <DropdownMenu>
                <Tooltip>
                  <DropdownMenuTrigger
                    render={
                      <TooltipTrigger
                        render={
                          <Button
                            variant="outline"
                            size="sm"
                            className="h-8 w-8 gap-1 px-0 text-muted-foreground md:w-auto md:px-2.5"
                          >
                            {isCompact ? (
                              <Rows3 className="size-3.5" />
                            ) : (
                              <LayoutGrid className="size-3.5" />
                            )}
                            <span className="hidden md:inline">
                              {isCompact ? t(($) => $.page.view_table) : t(($) => $.page.view_cards)}
                            </span>
                          </Button>
                        }
                      />
                    }
                  />
                  <TooltipContent side="bottom">{t(($) => $.toolbar.view)}</TooltipContent>
                </Tooltip>
                <DropdownMenuContent align="end" className="w-auto">
                  <DropdownMenuGroup>
                    <DropdownMenuLabel>{t(($) => $.toolbar.view)}</DropdownMenuLabel>
                  </DropdownMenuGroup>
                  <DropdownMenuRadioGroup
                    value={viewMode}
                    onValueChange={(v) => setViewMode(v as ProjectViewMode)}
                  >
                    <DropdownMenuRadioItem value="compact">
                      <Rows3 />
                      {t(($) => $.page.view_table)}
                    </DropdownMenuRadioItem>
                    <DropdownMenuRadioItem value="comfortable">
                      <LayoutGrid />
                      {t(($) => $.page.view_cards)}
                    </DropdownMenuRadioItem>
                  </DropdownMenuRadioGroup>
                </DropdownMenuContent>
              </DropdownMenu>
            </div>
          </div>

          {/* Body */}
          {!isLoading && visible.length === 0 ? (
            <div className="flex flex-1 flex-col items-center justify-center py-24 text-muted-foreground">
              <Search className="mb-3 h-10 w-10 opacity-30" />
              <p className="text-body">{t(($) => $.page.no_matches)}</p>
            </div>
          ) : isCompact ? (
            <div
              className="min-h-0 flex-1 overflow-hidden border border-transparent @container"
              onClick={(event) => handleGridMouseEvent(event, "click")}
              onAuxClick={(event) => handleGridMouseEvent(event, "auxclick")}
              onMouseOver={handleGridMouseOver}
            >
              <DataGrid
                className="h-full"
                table={table}
                recordCount={filteredProjects.length}
                isLoading={isLoading}
                emptyMessage={t(($) => $.page.no_matches)}
                tableLayout={{
                  dense: true,
                  width: "fixed",
                  rowBorder: true,
                  cellBorder: false,
                  headerBorder: true,
                  headerBackground: false,
                  columnsVisibility: true,
                }}
                tableClassNames={{
                  base: "border-transparent",
                  headerRow: "[&>th]:border-b-transparent",
                  bodyRow: "group/row cursor-pointer border-b-transparent [&>td]:border-b-transparent",
                  edgeCell: "border-transparent",
                }}
              >
                <DataGridScrollArea className="h-full" orientation="both">
                  <DataGridTable />
                </DataGridScrollArea>
              </DataGrid>
            </div>
          ) : isLoading ? (
            <LoadingState isCompact={isCompact} />
          ) : (
            <div className={cn("min-h-0 flex-1 overflow-y-auto pt-4", PAGE_GUTTER)}>
              <div
                className="grid grid-cols-1 gap-3 sm:grid-cols-2 lg:grid-cols-4"
                style={{ paddingBottom: "var(--spacing-6)" }}
              >
                {visible.map((project) => (
                  <ProjectCard
                    key={project.id}
                    project={project}
                    pinned={pinnedProjectIds.has(project.id)}
                    canDelete={isWorkspaceAdmin}
                  />
                ))}
              </div>
            </div>
          )}

          <ProjectBatchToolbar
            rows={selectedProjects}
            pinnedIds={pinnedProjectIds}
            canDelete={isWorkspaceAdmin}
            onClear={() => setSelectedIds(new Set())}
          />
        </>
      )}
    </div>
  );
}

function LoadingState({ isCompact }: { isCompact: boolean }) {
  if (isCompact) {
    return (
      <div className={cn("min-h-0 flex-1 overflow-auto pt-4", PAGE_GUTTER)}>
        <div className="space-y-2">
          {Array.from({ length: 6 }).map((_, i) => (
            <Skeleton key={i} className="h-11 w-full rounded-md" />
          ))}
        </div>
      </div>
    );
  }
  return (
    <div className={cn("grid grid-cols-1 gap-3 pt-4 sm:grid-cols-2 lg:grid-cols-4", PAGE_GUTTER)}>
      {Array.from({ length: 8 }).map((_, i) => (
        <div key={i} className="flex flex-col gap-2 rounded-md border p-3">
          <div className="flex items-center gap-2">
            <Skeleton className="h-8 w-8 rounded" />
            <Skeleton className="h-4 w-3/4" />
          </div>
          <div className="flex gap-1.5">
            <Skeleton className="h-5 w-16 rounded" />
            <Skeleton className="h-5 w-20 rounded" />
          </div>
        </div>
      ))}
    </div>
  );
}
