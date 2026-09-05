"use client";

import { useCallback, memo, type ReactNode, type SyntheticEvent } from "react";
import { AppLink } from "../../navigation";
import { useSortable, defaultAnimateLayoutChanges } from "@dnd-kit/sortable";
import type { AnimateLayoutChanges } from "@dnd-kit/sortable";
import { CSS } from "@dnd-kit/utilities";
import type { Issue, IssueProperty, Project, UpdateIssueRequest } from "@patchbay/core/types";
import { useQuery } from "@tanstack/react-query";
import { useWorkspaceId } from "@patchbay/core/hooks";
import { propertyListOptions } from "@patchbay/core/properties";
import { CustomPropertyValueDisplay } from "./pickers/custom-property-picker";
import { descriptionPreview } from "./description-preview";
import { formatDateOnly, isPastDateOnly } from "@patchbay/core/issues/date";
import { ActorAvatar } from "../../common/actor-avatar";
import { PropertyIcon } from "../../common/property-icon";
import { useWorkspacePaths } from "@patchbay/core/paths";
import { useLocale, useT } from "../../i18n";
import { ProjectIcon } from "../../projects/components/project-icon";
import { PriorityIcon } from "./priority-icon";
import { PriorityPicker, OwnerPicker, ExecutorPicker, StartDatePicker, DueDatePicker } from "./pickers";
import { useViewStore } from "@patchbay/core/issues/stores/view-store-context";
import { ProgressRing } from "./progress-ring";
import type { ChildProgress } from "./list-row";
import { IssueActionsContextMenu } from "../actions";
import { LabelChip } from "../../labels/label-chip";
import { IssueAgentActivityIndicator } from "./issue-agent-activity-indicator";
import { CustomStatusChip, useIsCustomStatus } from "./custom-status-chip";
import { useIssueSurfaceActionsOptional } from "../surface/actions-context";
import { cn } from "@patchbay/ui/lib/utils";
import { AVATAR_SIZE_PX } from "@patchbay/ui/lib/avatar-size";

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

const HOVER_REVEAL_FLEX_CLASS =
  "hidden group-hover/card:inline-flex group-data-[popup-open]/card:inline-flex focus-within:inline-flex has-[[data-open]]:inline-flex has-[[data-popup-open]]:inline-flex [@media(hover:none)]:inline-flex";

const BOARD_ACTOR_SIZE = "xs" as const;
// xs heads are 16px; 30% (the sm-stack default) only shifts 5px and still
// reads as two adjacent dots. Half-overlap is what makes owner+executor a
// single stacked chip in the identifier row.
const BOARD_ACTOR_OVERLAP_PX = Math.round(AVATAR_SIZE_PX[BOARD_ACTOR_SIZE] * 0.5);

function UnassignedActorSlot({ label }: { label: string }) {
  return (
    <span
      className="flex size-4 shrink-0 rounded-full border border-dashed border-muted-foreground/50"
      aria-label={label}
    />
  );
}

function BoardActorStackSlot({
  revealOnHover,
  peekLeft,
  overlapLeft,
  children,
}: {
  revealOnHover: boolean;
  peekLeft: boolean;
  overlapLeft: boolean;
  children: ReactNode;
}) {
  return (
    <span
      className={cn(
        "inline-flex rounded-full ring-2 ring-surface",
        peekLeft ? "absolute z-[1]" : "relative z-[2]",
        revealOnHover && HOVER_REVEAL_FLEX_CLASS,
      )}
      style={
        peekLeft
          ? { right: "100%", marginRight: -BOARD_ACTOR_OVERLAP_PX }
          : { marginLeft: overlapLeft ? -BOARD_ACTOR_OVERLAP_PX : 0 }
      }
    >
      {children}
    </span>
  );
}

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
  const isNonePriority = issue.priority === "none";
  const showDescription = storeProperties.description && issue.description;
  const showActorStack = storeProperties.executor;
  const hasOwner = !!issue.owner_type && !!issue.owner_id;
  const hasExecutor = !!issue.executor_type && !!issue.executor_id;
  const showAssignedOwner = showActorStack && hasOwner;
  const showUnassignedOwner = showActorStack && !hasOwner && canEdit;
  const showAssignedExecutor = showActorStack && hasExecutor;
  const showUnassignedExecutor = showActorStack && !hasExecutor && canEdit;
  const showStartDate = storeProperties.startDate && issue.start_date;
  const showDueDate = storeProperties.dueDate && issue.due_date;
  const showCreatedDate = !showStartDate && !showDueDate;
  const showProject = storeProperties.project && project;
  const showChildProgress = storeProperties.childProgress && childProgress;
  const showLabels = storeProperties.labels && labels.length > 0;
  // Keeps the chip row from rendering an empty flex container when the status
  // chip is the only thing in it and it decides to render nothing.
  const showCustomStatus = useIsCustomStatus(issue.status);

  const priorityLabel = t(($) => $.priority[issue.priority]);
  const showPriorityControl = showPriority && (!isNonePriority || canEdit);
  const showVisibleChipRow =
    showCustomStatus ||
    showProject ||
    showLabels ||
    cardCustomProperties.length > 0 ||
    (showPriority && !isNonePriority);
  const showHoverOnlyPriority = !!showPriorityControl && isNonePriority && !showVisibleChipRow;
  const showChipRow = showVisibleChipRow || showHoverOnlyPriority;
  const priorityIconNode = showPriorityControl ? (
    canEdit ? (
      <PickerWrapper
        className={cn(
          "flex",
          isNonePriority &&
            "hidden group-hover/card:flex group-data-[popup-open]/card:flex focus-within:flex has-[[data-open]]:flex has-[[data-popup-open]]:flex [@media(hover:none)]:flex",
        )}
      >
        <PriorityPicker
          priority={issue.priority}
          onUpdate={handleUpdate}
          triggerRender={
            <button
              type="button"
              aria-label={priorityLabel}
              className="inline-flex size-5 shrink-0 items-center justify-center rounded hover:bg-muted/60"
            >
              <PriorityIcon priority={issue.priority} />
            </button>
          }
        />
      </PickerWrapper>
    ) : (
      <span
        aria-label={priorityLabel}
        className="inline-flex size-5 shrink-0 items-center justify-center"
      >
        <PriorityIcon priority={issue.priority} />
      </span>
    )
  ) : null;

  const ownerInner = showAssignedOwner ? (
    <ActorAvatar
      actorType={issue.owner_type!}
      actorId={issue.owner_id!}
      size={BOARD_ACTOR_SIZE}
      enableHoverCard
      profileLink={false}
      className="shrink-0"
    />
  ) : showUnassignedOwner ? (
    <UnassignedActorSlot label={t(($) => $.pickers.owner.trigger_unassigned)} />
  ) : null;

  const executorInner = showAssignedExecutor ? (
    <ActorAvatar
      actorType={issue.executor_type!}
      actorId={issue.executor_id!}
      size={BOARD_ACTOR_SIZE}
      enableHoverCard
      profileLink={false}
      className="shrink-0"
    />
  ) : showUnassignedExecutor ? (
    <UnassignedActorSlot label={t(($) => $.pickers.executor.trigger_unassigned)} />
  ) : null;

  const wrapActorPicker = (inner: ReactNode, picker: ReactNode) =>
    canEdit ? (
      <PickerWrapper className="inline-flex items-center">{picker}</PickerWrapper>
    ) : (
      <span className="inline-flex items-center">{inner}</span>
    );

  const actorSlots: { key: "owner" | "executor"; node: ReactNode; revealOnHover: boolean }[] = [];
  if (ownerInner) {
    actorSlots.push({
      key: "owner",
      revealOnHover: showUnassignedOwner && showAssignedExecutor,
      node: wrapActorPicker(
        ownerInner,
        <OwnerPicker
          ownerType={issue.owner_type}
          ownerId={issue.owner_id}
          onUpdate={handleUpdate}
          trigger={ownerInner}
        />,
      ),
    });
  }
  if (executorInner) {
    actorSlots.push({
      key: "executor",
      revealOnHover: showUnassignedExecutor && showAssignedOwner,
      node: wrapActorPicker(
        executorInner,
        <ExecutorPicker
          executorType={issue.executor_type}
          executorId={issue.executor_id}
          onUpdate={handleUpdate}
          trigger={executorInner}
        />,
      ),
    });
  }

  const onlyUnassignedActors =
    !showAssignedOwner &&
    !showAssignedExecutor &&
    (showUnassignedOwner || showUnassignedExecutor);

  const actorStack =
    actorSlots.length > 0 ? (
      <span
        data-board-actor-stack=""
        className={cn(
          "relative inline-flex items-center",
          onlyUnassignedActors && HOVER_REVEAL_OPACITY_CLASS,
        )}
      >
        {actorSlots.map((slot, index) => {
          const inFlowIndex = actorSlots.slice(0, index).filter((item) => !item.revealOnHover).length;
          return (
            <BoardActorStackSlot
              key={slot.key}
              revealOnHover={slot.revealOnHover}
              peekLeft={slot.revealOnHover && index === 0}
              overlapLeft={slot.revealOnHover || inFlowIndex > 0}
            >
              {slot.node}
            </BoardActorStackSlot>
          );
        })}
      </span>
    ) : null;

  const showMetaRow =
    showCreatedDate || !!showStartDate || !!showDueDate || !!showChildProgress;

  return (
    <div className="rounded-lg border-[0.5px] border-surface-border bg-surface py-2 px-2.5 shadow-[var(--surface-shadow)] transition-colors group-hover/card:border-foreground/15 group-hover/card:bg-surface-hover group-data-[popup-open]/card:border-foreground/15 group-data-[popup-open]/card:bg-surface-hover">
      {/* Row 1: identifier (left), activity + owner/executor stack (right) */}
      <div className="flex items-center justify-between gap-2">
        <p className="min-w-0 truncate text-caption text-muted-foreground">{issue.identifier}</p>
        <div className="flex shrink-0 items-center gap-1.5">
          <IssueAgentActivityIndicator issueId={issue.id} hideAvatars />
          {actorStack}
        </div>
      </div>

      {/* Row 2: Title */}
      <p className="mt-1 text-body font-medium leading-snug line-clamp-2">
        {issue.title}
      </p>

      {showDescription && (() => {
        const preview = descriptionPreview(issue.description!);
        if (!preview) return null;
        return (
          <p className="mt-1 text-caption text-muted-foreground line-clamp-1">
            {preview}
          </p>
        );
      })()}

      {/* Chip row: priority + status + project + labels + custom property values.
          Priority sits with labels the way Linear does, not next to the id.
          The status chip renders only for a CUSTOM status — the column header
          already names the category. (MUL-6243) */}
      {showChipRow && (
        <div
          data-board-chip-row=""
          className={cn(
            "mt-1.5 items-center gap-1.5 flex-wrap",
            showHoverOnlyPriority ? HOVER_REVEAL_FLEX_CLASS : "flex",
          )}
        >
          {priorityIconNode}
          <CustomStatusChip status={issue.status} />
          {showProject && (
            <span className="inline-flex items-center gap-1 text-micro text-muted-foreground max-w-[160px]">
              <ProjectIcon project={project} size="sm" />
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
      )}

      {/* Meta row: dates (left), child progress (right) */}
      {showMetaRow && (
        <div className="mt-1.5 flex items-center gap-2">
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
                          {formatDate(issue.start_date!, locale)}
                        </span>
                      }
                    />
                  </PickerWrapper>
                ) : (
                  <span className="truncate text-caption text-muted-foreground">
                    {formatDate(issue.start_date!, locale)}
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
                          {formatDate(issue.due_date!, locale)}
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
                    {formatDate(issue.due_date!, locale)}
                  </span>
                )
              )}
              {showCreatedDate && (
                <span className="truncate text-caption text-muted-foreground">
                  {formatDate(issue.created_at, locale)}
                </span>
              )}
            </div>
          )}
          {showChildProgress && (
            <div className="ml-auto inline-flex shrink-0 items-center gap-1">
              <ProgressRing done={childProgress!.done} total={childProgress!.total} size={14} />
              <span className="text-micro text-muted-foreground tabular-nums font-medium">
                {childProgress!.done}/{childProgress!.total}
              </span>
            </div>
          )}
        </div>
      )}
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
