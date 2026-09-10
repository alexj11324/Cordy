"use client";

import { useCallback, memo, type ReactNode, type SyntheticEvent } from "react";
import { AppLink } from "../../navigation";
import { UserRound } from "lucide-react";
import { Avatar, AvatarFallback } from "@orvilo/ui/components/ui/avatar";
import { useSortable, defaultAnimateLayoutChanges } from "@dnd-kit/sortable";
import type { AnimateLayoutChanges } from "@dnd-kit/sortable";
import { CSS } from "@dnd-kit/utilities";
import type { Issue, IssueProperty, Project, UpdateIssueRequest } from "@orvilo/core/types";
import { useQuery } from "@tanstack/react-query";
import { useWorkspaceId } from "@orvilo/core/hooks";
import { propertyListOptions } from "@orvilo/core/properties";
import { CustomPropertyValueDisplay } from "./pickers/custom-property-picker";
import { descriptionPreview } from "./description-preview";
import { formatDateOnly, isPastDateOnly } from "@orvilo/core/issues/date";
import { ActorAvatar } from "../../common/actor-avatar";
import { PropertyIcon } from "../../common/property-icon";
import { useWorkspacePaths } from "@orvilo/core/paths";
import { useLocale, useT } from "../../i18n";
import { ProjectIcon } from "../../projects/components/project-icon";
import { PriorityIcon } from "./priority-icon";
import { StatusIcon } from "./status-icon";
import { statusCategoryOfKey } from "@orvilo/core/issues";
import { KanbanItem, KanbanItemHandle } from "@orvilo/ui/components/reui/kanban";

export const BOARD_CARD_CONTENT_WIDTH = 304;
import { PriorityPicker, ExecutorPicker, StartDatePicker, DueDatePicker } from "./pickers";
import { useViewStore } from "@orvilo/core/issues/stores/view-store-context";
import { ProgressRing } from "./progress-ring";
import type { ChildProgress } from "./list-row";
import { IssueActionsContextMenu } from "../actions";
import { LabelChip } from "../../labels/label-chip";
import { IssueAgentActivityIndicator } from "./issue-agent-activity-indicator";
import { CustomStatusChip } from "./custom-status-chip";
import { useIssueSurfaceActionsOptional } from "../surface/actions-context";
import { cn } from "@orvilo/ui/lib/utils";
import { shouldStopBoardCardDragKey } from "./board-card-keyboard";

function formatDate(date: string, locale: string): string {
  return formatDateOnly(date, { month: "short", day: "numeric" }, locale);
}

/** Stops event from bubbling to Link/drag handlers */
function PickerWrapper({ children, className }: { children: ReactNode; className?: string }) {
  const stop = (e: SyntheticEvent) => {
    e.stopPropagation();
    e.preventDefault();
  };
  return (
    <div onClick={stop} onMouseDown={stop} onPointerDown={stop} className={className}>
      {children}
    </div>
  );
}

const HOVER_REVEAL_OPACITY_CLASS =
  "opacity-0 transition-opacity group-hover/card:opacity-100 group-data-[popup-open]/card:opacity-100 focus-within:opacity-100 has-[[data-open]]:opacity-100 has-[[data-popup-open]]:opacity-100 [@media(hover:none)]:opacity-100";

export const BoardCardContent = memo(function BoardCardContent({
  issue,
  editable = false,
  childProgress,
  project,
}: {
  issue: Issue;
  editable?: boolean;
  childProgress?: ChildProgress;
  project?: Project;
}) {
  const { t } = useT("issues");
  const locale = useLocale();
  const storeProperties = useViewStore((s) => s.cardProperties);
  const cardPropertyIds = useViewStore((s) => s.cardPropertyIds);
  const cardWsId = useWorkspaceId();
  const { data: workspaceProperties = [] } = useQuery(propertyListOptions(cardWsId));
  // Custom properties toggled on in Display options, in toggle order, only
  // when this issue actually carries a value.
  const cardCustomProperties = cardPropertyIds
    .map((id) => workspaceProperties.find((p) => p.id === id))
    .filter((p): p is IssueProperty => !!p && issue.properties?.[p.id] !== undefined);
  const labels = issue.labels ?? [];

  const surfaceActions = useIssueSurfaceActionsOptional();
  const handleUpdate = useCallback(
    (updates: Partial<UpdateIssueRequest>) => {
      surfaceActions?.updateIssue(issue.id, updates, {
        errorMessage: t(($) => $.card.update_failed),
      });
    },
    [issue.id, surfaceActions, t],
  );
  const canEdit = editable && !!surfaceActions;

  const showPriority = storeProperties.priority;
  const showDescription = storeProperties.description;
  const showExecutorSection = storeProperties.executor;
  const hasExecutor = !!issue.executor_type && !!issue.executor_id;
  const showAssignedExecutor = showExecutorSection && hasExecutor;
  const showUnassignedAssign = showExecutorSection && !hasExecutor && canEdit;
  const showStartDate = storeProperties.startDate && issue.start_date;
  const showDueDate = storeProperties.dueDate && issue.due_date;
  const showCreatedDate = !showStartDate && !showDueDate;
  const showProject = storeProperties.project && project;
  const showChildProgress = storeProperties.childProgress && childProgress;
  const showLabels = storeProperties.labels && labels.length > 0;
  const priorityLabel = t(($) => $.priority[issue.priority]);
  const showPriorityControl = showPriority;
  const priorityIconNode = showPriorityControl ? (
    canEdit ? (
      <PickerWrapper className="flex">
        <PriorityPicker
          priority={issue.priority}
          onUpdate={handleUpdate}
          triggerRender={
            <button
              type="button"
              aria-label={priorityLabel}
              className="inline-flex size-5 shrink-0 items-center justify-center rounded hover:bg-muted/60"
            >
              <PriorityIcon priority={issue.priority} className="size-4!" />
            </button>
          }
        />
      </PickerWrapper>
    ) : (
      <span
        aria-label={priorityLabel}
        className="inline-flex size-5 shrink-0 items-center justify-center"
      >
        <PriorityIcon priority={issue.priority} className="size-4!" />
      </span>
    )
  ) : null;

  const assignedExecutor = showAssignedExecutor ? (
    <span className="flex shrink-0 items-center">
      <ActorAvatar
        actorType={issue.executor_type!}
        actorId={issue.executor_id!}
        size="md"
        enableHoverCard
        profileLink={false}
        className="shrink-0"
      />
    </span>
  ) : null;

  const unassignedAssign = showUnassignedAssign ? (
    <span
      className="flex size-4 shrink-0 rounded-full border border-dashed border-muted-foreground/50"
      aria-label={t(($) => $.pickers.executor.trigger_unassigned)}
    />
  ) : null;

  const executorInner = assignedExecutor ?? unassignedAssign;

  const executorNode = executorInner ? (
    canEdit ? (
      <PickerWrapper className={cn("inline-flex items-center", showUnassignedAssign && HOVER_REVEAL_OPACITY_CLASS)}>
        <ExecutorPicker
          executorType={issue.executor_type}
          executorId={issue.executor_id}
          onUpdate={handleUpdate}
          trigger={executorInner}
        />
      </PickerWrapper>
    ) : (
      <span className="inline-flex items-center">{executorInner}</span>
    )
  ) : null;

  return (
    <div className="running-task-card border-beam rounded-lg border-[0.5px] border-surface-border bg-surface py-2 px-2.5 shadow-[var(--surface-shadow)] transition-colors hover:border-foreground/15 hover:bg-surface-hover focus-within:border-foreground/15 focus-within:bg-surface-hover group-data-[popup-open]/card:border-foreground/15 group-data-[popup-open]/card:bg-surface-hover">
      {/* Identifier and assigned executor; live activity remains in the footer. */}
      <div data-board-identifier-row="" className="flex min-h-6 items-center justify-between gap-2">
        <p className="min-w-0 truncate text-caption text-muted-foreground">{issue.identifier}</p>
        {executorNode}
      </div>

      {/* Row 2: Title */}
      <div data-board-title-row="" className="mt-1 flex items-start gap-1.5">
        <span aria-label={issue.status_name ?? t(($) => $.status[issue.status_category ?? statusCategoryOfKey(issue.status)])}>
          <StatusIcon status={issue.status} category={issue.status_category} className="mt-0.5 size-3.5" />
        </span>
        <p className="min-w-0 text-body font-medium leading-snug line-clamp-2">{issue.title}</p>
      </div>

      {showDescription && (() => {
        const preview = descriptionPreview(issue.description ?? "");
        return (
          <p className="mt-1 min-h-4 text-caption text-muted-foreground line-clamp-1">
            {preview || "—"}
          </p>
        );
      })()}

      {/* Chip row: priority + custom status + project + labels + custom values.
          Built-in category status is the column header, not a second glyph. */}
      <div data-board-chip-row="" className="mt-1.5 flex min-h-5 flex-wrap items-center gap-1.5">
          {priorityIconNode}
          <CustomStatusChip status={issue.status} />
          {showProject && (
            <span className="inline-flex items-center gap-1.5 text-label text-foreground max-w-[160px]">
              <ProjectIcon project={project} size="md" />
              <span className="truncate">{project!.title}</span>
            </span>
          )}
          {showLabels && labels.map((label) => (
            <LabelChip key={label.id} label={label} variant="dot" />
          ))}
          {cardCustomProperties.map((property) => (
            <span
              key={property.id}
              className="inline-flex max-w-[160px] items-center gap-1 text-micro text-muted-foreground"
            >
              <PropertyIcon property={property} className="size-3 text-micro" />
              <CustomPropertyValueDisplay property={property} value={issue.properties?.[property.id]} />
            </span>
          ))}
      </div>

      {/* Meta row: human owner and dates (left), child progress and live activity (right) */}
      <div data-board-meta-row="" className="mt-1.5 flex min-h-6 items-center gap-2">
        <span data-board-owner="" className="inline-flex shrink-0">
          {issue.owner_type === "member" && issue.owner_id ? (
            <ActorAvatar actorType="member" actorId={issue.owner_id} size="md" enableHoverCard profileLink={false} />
          ) : (
            <Avatar size="sm" className="size-6" aria-label={t(($) => $.pickers.owner.trigger_unassigned)}>
              <AvatarFallback><UserRound className="size-4" aria-hidden="true" /></AvatarFallback>
            </Avatar>
          )}
        </span>
          {(showStartDate || showDueDate || showCreatedDate) && (
            <div className="flex min-w-0 flex-1 items-center gap-2">
              {showStartDate && (
                canEdit ? (
                  <PickerWrapper className="flex min-w-0">
                    <StartDatePicker
                      startDate={issue.start_date}
                      onUpdate={handleUpdate}
                      trigger={
                        <span className="truncate text-caption text-muted-foreground">
                          {t(($) => $.card.starts_on, { date: formatDate(issue.start_date!, locale) })}
                        </span>
                      }
                    />
                  </PickerWrapper>
                ) : (
                  <span className="truncate text-caption text-muted-foreground">
                    {t(($) => $.card.starts_on, { date: formatDate(issue.start_date!, locale) })}
                  </span>
                )
              )}
              {showDueDate && (
                canEdit ? (
                  <PickerWrapper className="flex min-w-0">
                    <DueDatePicker
                      dueDate={issue.due_date}
                      onUpdate={handleUpdate}
                      trigger={
                        <span
                          className={`truncate text-caption ${
                            isPastDateOnly(issue.due_date)
                              ? "text-destructive"
                              : "text-muted-foreground"
                          }`}
                        >
                          {t(($) => $.card.due_on, { date: formatDate(issue.due_date!, locale) })}
                        </span>
                      }
                    />
                  </PickerWrapper>
                ) : (
                  <span
                    className={`truncate text-caption ${
                      isPastDateOnly(issue.due_date)
                        ? "text-destructive"
                        : "text-muted-foreground"
                    }`}
                  >
                    {t(($) => $.card.due_on, { date: formatDate(issue.due_date!, locale) })}
                  </span>
                )
              )}
              {showCreatedDate && (
                <span className="truncate text-caption text-muted-foreground">
                  {t(($) => $.card.created_on, { date: formatDate(issue.created_at, locale) })}
                </span>
              )}
            </div>
          )}
          <div className="ml-auto flex shrink-0 items-center gap-1.5">
              {showChildProgress && (
                <div className="inline-flex shrink-0 items-center gap-1">
                  <ProgressRing done={childProgress!.done} total={childProgress!.total} size={14} />
                  <span className="text-micro text-muted-foreground tabular-nums font-medium">
                    {childProgress!.done}/{childProgress!.total}
                  </span>
                </div>
              )}
              <IssueAgentActivityIndicator issueId={issue.id} size="md" />
          </div>
      </div>
    </div>
  );
});

const animateLayoutChanges: AnimateLayoutChanges = (args) => {
  const { isSorting, wasDragging } = args;
  if (isSorting || wasDragging) return false;
  return defaultAnimateLayoutChanges(args);
};

export const DraggableBoardCard = memo(function DraggableBoardCard({
  issue,
  childProgress,
  project,
  disableSorting,
}: {
  issue: Issue;
  childProgress?: ChildProgress;
  project?: Project;
  disableSorting?: boolean;
}) {
  const p = useWorkspacePaths();
  const {
    attributes,
    listeners,
    setNodeRef,
    transform,
    transition,
    isDragging,
  } = useSortable({
    id: issue.id,
    data: { status: issue.status },
    animateLayoutChanges,
    disabled: disableSorting ? { droppable: true } : undefined,
  });

  const style = {
    transform: CSS.Transform.toString(transform),
    transition,
  };

  return (
    <IssueActionsContextMenu issue={issue}>
      <div
        ref={setNodeRef}
        style={style}
        data-board-card=""
        {...attributes}
        {...listeners}
        className={`group/card ${isDragging ? "opacity-30" : ""}`}
      >
        <AppLink
          href={p.issueDetail(issue.id)}
          newTabTitle={issue.identifier}
          className={`group block transition-colors ${isDragging ? "pointer-events-none" : ""}`}
          onKeyDown={(event) => {
            if (shouldStopBoardCardDragKey(event.key)) event.stopPropagation();
          }}
        >
          <BoardCardContent
            issue={issue}
            editable
            childProgress={childProgress}
            project={project}
          />
        </AppLink>
      </div>
    </IssueActionsContextMenu>
  );
});

/** ReUI's sortable item shell with Orvilo's real task card content. */
export const KanbanBoardCard = memo(function KanbanBoardCard({
  issue,
  childProgress,
  project,
}: {
  issue: Issue;
  childProgress?: ChildProgress;
  project?: Project;
}) {
  const p = useWorkspacePaths();

  return (
    <IssueActionsContextMenu issue={issue}>
      <KanbanItem
        value={issue.id}
        data-board-card=""
        className="group/card"
        tabIndex={-1}
      >
        <KanbanItemHandle className="block" tabIndex={0} role="button" aria-label={issue.title}>
          <AppLink
            href={p.issueDetail(issue.id)}
            newTabTitle={issue.identifier}
            className="group block transition-colors"
            onKeyDown={(event) => {
              if (shouldStopBoardCardDragKey(event.key)) event.stopPropagation();
            }}
          >
            <BoardCardContent
              issue={issue}
              editable
              childProgress={childProgress}
              project={project}
            />
          </AppLink>
        </KanbanItemHandle>
      </KanbanItem>
    </IssueActionsContextMenu>
  );
});
