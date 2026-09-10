"use client";

import { useState, useCallback, useMemo, useEffect, useRef, memo } from "react";
import { useQuery } from "@tanstack/react-query";
import { closestCenter, type CollisionDetection, type DragStartEvent, type DragEndEvent, type DragOverEvent } from "@dnd-kit/core";
import { arrayMove, horizontalListSortingStrategy, SortableContext } from "@dnd-kit/sortable";
import { toast } from "sonner";
import type {
  Issue,
  IssueExecutorType,
  IssueStatusCategory,
  Project,
  IssueProperty,
} from "@orvilo/core/types";
import {
  useViewStore,
  useViewStoreApi,
} from "@orvilo/core/issues/stores/view-store-context";
import { propertyIdFromViewKey } from "@orvilo/core/issues/stores/view-store";
import { propertyListOptions, useSetIssueProperty, useUnsetIssueProperty } from "@orvilo/core/properties";
import { useWorkspaceId } from "@orvilo/core/hooks";
import type { IssueGrouping } from "@orvilo/core/issues/stores/view-store";
import { useActorName } from "@orvilo/core/workspace/hooks";
import { BoardColumn, BOARD_CARD_WIDTH, type BoardColumnGroup } from "./board-column";
import { BoardCardContent } from "./board-card";
import { BoardScrollArea } from "./board-scroll-area";
import { InfiniteScrollSentinel } from "./infinite-scroll-sentinel";
import { ListLoadMoreFooter } from "./list-load-more-footer";
import type { ChildProgress } from "./list-row";
import type { IssueCreateDefaults } from "../surface/types";
import type {
  IssueStatusPageState,
  IssueStatusPagination,
} from "../surface/use-issue-status-branches";
import type { MoveIssueCallbacks } from "../surface/use-issue-surface-actions";
import type {
  IssueGroupBranches,
  IssueGroupPageState,
} from "../surface/use-issue-group-branches";
import { useDragSettle } from "./use-drag-settle";
import { useBoardDragPan } from "./use-board-drag-pan";
import { useT } from "../../i18n";
import {
  Kanban,
  KanbanOverlay,
  type KanbanMoveEvent,
} from "@orvilo/ui/components/reui/kanban";
import {
  type DragMoveUpdates,
  makeKanbanCollision,
  statusGroupId,
  executorGroupId,
  buildColumns,
  computePosition,
  findColumn,
  getMoveAnchors,
  insertIdByPosition,
  issueMatchesGroup,
  getMoveUpdates,
  propertyGroupId,
  projectGroupId,
} from "../utils/drag-utils";

function isStatusGroup(
  group: BoardColumnGroup,
): group is BoardColumnGroup & { status: IssueStatusCategory } {
  return group.status !== undefined;
}

function makeStatusGroup(status: IssueStatusCategory): BoardColumnGroup {
  return {
    id: statusGroupId(status),
    title: status,
    status,
    createData: { status },
  };
}

interface ProjectColumnLabels {
  noProject: string;
  /** A project id the projects query cannot resolve — deleted, or not visible
   *  to this member. Shares the Table's wording so one board column and one
   *  table group row never describe the same project differently. */
  unavailableProject: string;
}

interface BuildGroupsContext extends ProjectColumnLabels {
  getActorName: (type: string, id: string) => string;
  groupingProperty: IssueProperty | null;
  projectMap: Map<string, Project> | undefined;
  noExecutorLabel: string;
  noValueLabel: string;
}

/**
 * One project column. Shared by the client fallback (columns derived from
 * loaded cards) and the server path (columns derived from group descriptors)
 * so the two can never describe the same project differently.
 */
function projectColumn(
  id: string,
  projectId: string | null,
  projectMap: Map<string, Project> | undefined,
  labels: ProjectColumnLabels,
  totalCount?: number,
): BoardColumnGroup {
  const project = projectId ? projectMap?.get(projectId) ?? null : null;
  return {
    id,
    title: projectId
      ? project?.title ?? labels.unavailableProject
      : labels.noProject,
    projectId,
    project,
    totalCount,
    createData: { project_id: projectId },
  };
}

function createDataForExecutor(
  actor: { type: IssueExecutorType; id: string } | null,
): IssueCreateDefaults {
  if (!actor) {
    return {
      executor_type: null,
      executor_id: null,
    };
  }
  return { executor_type: actor.type, executor_id: actor.id };
}

/**
 * Keep the "No project" column present as a drop target — clearing a card's
 * project by dragging has to stay possible even in a workspace where every
 * card currently has one. A board with no columns at all is left alone: that
 * is the surface's empty state, not a board missing one column.
 */
function withNoProjectColumn(
  columns: BoardColumnGroup[],
  projectMap: Map<string, Project> | undefined,
  labels: ProjectColumnLabels,
): BoardColumnGroup[] {
  if (columns.length === 0) return columns;
  if (columns.some((column) => column.projectId === null)) return columns;
  // No-project sorts first server-side, so it is always in the first page of
  // descriptors when it exists — an absent one cannot arrive with a later page.
  return [
    projectColumn(projectGroupId(null), null, projectMap, labels, 0),
    ...columns,
  ];
}

function buildGroups(
  issues: Issue[],
  visibleStatuses: IssueStatusCategory[],
  grouping: IssueGrouping,
  {
    getActorName,
    groupingProperty,
    projectMap,
    noExecutorLabel,
    noValueLabel,
    ...projectLabels
  }: BuildGroupsContext,
): BoardColumnGroup[] {
  if (grouping === "status") {
    return visibleStatuses.map(makeStatusGroup);
  }

  // Select-property board: one column per option (definition order) plus a
  // trailing "No value" column. Empty columns stay visible — they are drop
  // targets for assigning the value.
  if (groupingProperty) {
    const columns: BoardColumnGroup[] = (groupingProperty.config.options ?? []).map(
      (option) => ({
        id: propertyGroupId(groupingProperty.id, option.id),
        title: option.name,
        propertyId: groupingProperty.id,
        propertyOptionId: option.id,
        propertyOptionColor: option.color,
      }),
    );
    columns.push({
      id: propertyGroupId(groupingProperty.id, null),
      title: noValueLabel,
      propertyId: groupingProperty.id,
      propertyOptionId: null,
    });
    return columns;
  }

  // Project board: one column per project the loaded cards reference, plus the
  // "No project" column. Ordering mirrors the server's group order (no-project
  // first, then project title) so the client fallback and the paged server
  // columns cannot disagree.
  if (grouping === "project") {
    const columns = new Map<string, BoardColumnGroup>();
    for (const issue of issues) {
      const projectId = issue.project_id ?? null;
      const id = projectGroupId(projectId);
      if (columns.has(id)) continue;
      columns.set(id, projectColumn(id, projectId, projectMap, projectLabels));
    }
    const ordered = Array.from(columns.values()).toSorted((a, b) => {
      if (a.projectId === null) return b.projectId === null ? 0 : -1;
      if (b.projectId === null) return 1;
      return a.title.localeCompare(b.title);
    });
    return withNoProjectColumn(ordered, projectMap, projectLabels);
  }

  const groups = new Map<string, BoardColumnGroup>();
  for (const issue of issues) {
    const executorType = issue.executor_type;
    const executorId = issue.executor_id;
    const id = executorGroupId(executorType, executorId);
    if (groups.has(id)) continue;

    if (executorType && executorId) {
      groups.set(id, {
        id,
        title: getActorName(executorType, executorId),
        executorType,
        executorId,
        createData: createDataForExecutor({ type: executorType, id: executorId }),
      });
      continue;
    }

    groups.set(id, {
      id,
      title: noExecutorLabel,
      executorType: null,
      executorId: null,
      createData: createDataForExecutor(null),
    });
  }

  const order: Record<string, number> = {
    agent: 0,
    team: 1,
    none: 2,
  };

  return Array.from(groups.values()).toSorted((a, b) => {
    const aOrder = order[a.executorType ?? "none"] ?? 99;
    const bOrder = order[b.executorType ?? "none"] ?? 99;
    if (aOrder !== bOrder) return aOrder - bOrder;
    return a.title.localeCompare(b.title);
  });
}

const EMPTY_PROGRESS_MAP = new Map<string, ChildProgress>();
const EMPTY_IDS: string[] = [];
const issueId = (id: string) => id;

function preserveColumnOrder(previous: Record<string, string[]>, refreshed: Record<string, string[]>) {
  return Object.fromEntries(
    [...new Set([...Object.keys(previous), ...Object.keys(refreshed)])]
      .filter((key) => key in refreshed)
      .map((key) => [key, refreshed[key]!]),
  );
}

function BoardViewImpl({
  issues,
  visibleStatuses,
  hiddenStatuses,
  droppableHiddenStatuses,
  onMoveIssue,
  childProgressMap = EMPTY_PROGRESS_MAP,
  projectMap,
  projectId,
  onCreateIssue,
  statusPagination,
  groupBranches,
}: {
  issues: Issue[];
  visibleStatuses: IssueStatusCategory[];
  hiddenStatuses: IssueStatusCategory[];
  /** Hidden by display settings, not excluded by the view filter. */
  droppableHiddenStatuses?: IssueStatusCategory[];
  onMoveIssue: (issueId: string, updates: DragMoveUpdates, callbacks?: MoveIssueCallbacks) => boolean | void;
  childProgressMap?: Map<string, ChildProgress>;
  projectMap?: Map<string, Project>;
  /** When set, the per-column "+" pre-fills the project on the create form. */
  projectId?: string;
  onCreateIssue?: (defaults: IssueCreateDefaults) => void;
  statusPagination?: IssueStatusPagination;
  groupBranches?: IssueGroupBranches;
}) {
  const { t } = useT("issues");
  const storeGrouping = useViewStore((s) => s.grouping);
  const sortBy = useViewStore((s) => s.sortBy);
  const viewStoreApi = useViewStoreApi();
  const boardWsId = useWorkspaceId();
  const { data: workspaceProperties = [] } = useQuery(propertyListOptions(boardWsId));
  const groupingPropertyId = propertyIdFromViewKey(storeGrouping);
  const groupingProperty = groupingPropertyId
    ? workspaceProperties.find((p) => p.id === groupingPropertyId && p.type === "select") ?? null
    : null;
  // A persisted `property:<id>` grouping whose definition is gone (archived,
  // deleted, other workspace) falls back to status columns.
  const grouping: IssueGrouping =
    groupingPropertyId && !groupingProperty ? "status" : storeGrouping;
  const groupingOptionIds = useMemo(
    () =>
      groupingProperty
        ? new Set((groupingProperty.config.options ?? []).map((option) => option.id))
        : undefined,
    [groupingProperty],
  );
  const setIssuePropertyMutation = useSetIssueProperty();
  const unsetIssuePropertyMutation = useUnsetIssueProperty();
  const applyPropertyGroupValue = useCallback(
    (group: BoardColumnGroup, issueId: string) => {
      if (group.propertyId === undefined) return;
      // Surface failures like status/executor drags do (use-issue-surface-
      // actions): the mutation rolls the card back, but without a toast the
      // snap-back reads as a UI glitch instead of a rejected write.
      const onError = (err: unknown) => {
        toast.error(
          err instanceof Error && err.message
            ? err.message
            : t(($) => $.page.move_failed),
        );
      };
      if (group.propertyOptionId === null) {
        unsetIssuePropertyMutation.mutate(
          { issueId, propertyId: group.propertyId },
          { onError },
        );
      } else if (group.propertyOptionId !== undefined) {
        setIssuePropertyMutation.mutate(
          {
            issueId,
            propertyId: group.propertyId,
            value: group.propertyOptionId,
          },
          { onError },
        );
      }
    },
    [setIssuePropertyMutation, t, unsetIssuePropertyMutation],
  );
  const sortFieldKey = sortBy === "created_at" ? "created" : sortBy;
  const sortPropertyId = propertyIdFromViewKey(sortBy);
  const sortLabel = sortBy !== "position"
    ? t(($) => $.board.ordered_by, {
        field: sortPropertyId
          ? workspaceProperties.find((p) => p.id === sortPropertyId)?.name ?? ""
          : t(($) => $.display[`sort_${sortFieldKey}` as keyof typeof $.display]),
      })
    : null;
  const { getActorName } = useActorName();
  const groupedIssues = useMemo(
    () => (groupBranches?.enabled ? groupBranches.issues : issues),
    [groupBranches, issues],
  );
  const hydratedExecutorGroups = useMemo<BoardColumnGroup[] | undefined>(() => {
    if (grouping === "executor" && groupBranches?.enabled) {
      return groupBranches.descriptors.flatMap((descriptor): BoardColumnGroup[] => {
        if (descriptor.value.kind !== "executor") return [];
        const actorRef = descriptor.value.actor;
        const actor: { type: IssueExecutorType; id: string } | null =
          actorRef &&
          (actorRef.type === "agent" ||
            actorRef.type === "team")
            ? { type: actorRef.type, id: actorRef.id }
            : null;
        return [{
          id: descriptor.key,
          title: actor
            ? getActorName(actor.type, actor.id)
            : t(($) => $.filters.no_executor),
          executorType: actor?.type ?? null,
          executorId: actor?.id ?? null,
          totalCount: descriptor.count,
          createData: createDataForExecutor(actor),
        }];
      });
    }
    return undefined;
  }, [getActorName, groupBranches, grouping, t]);
  const projectColumnLabels = useMemo<ProjectColumnLabels>(
    () => ({
      noProject: t(($) => $.swimlane.no_project),
      unavailableProject: t(($) => $.table.value_unavailable),
    }),
    [t],
  );
  const hydratedProjectGroups = useMemo<BoardColumnGroup[] | undefined>(() => {
    if (grouping !== "project" || !groupBranches?.enabled) return undefined;
    const columns = groupBranches.descriptors.flatMap(
      (descriptor): BoardColumnGroup[] =>
        descriptor.value.kind === "project"
          ? [
              projectColumn(
                // The descriptor key, not our own: it is what `groupPagination`
                // is keyed by. `projectGroupId` reproduces it exactly, which is
                // what lets cards bucket into these columns at all.
                descriptor.key,
                descriptor.value.project_id ?? null,
                projectMap,
                projectColumnLabels,
                descriptor.count,
              ),
            ]
          : [],
    );
    return withNoProjectColumn(columns, projectMap, projectColumnLabels);
  }, [groupBranches, grouping, projectColumnLabels, projectMap]);
  const groupPagination = useMemo(() => {
    if (!groupBranches?.enabled) return undefined;
    const grouped = new Map<string, IssueGroupPageState[]>();
    for (const descriptor of groupBranches.descriptors) {
      const page = groupBranches.pagination[descriptor.key];
      if (!page) continue;
      let id = descriptor.key;
      if (descriptor.value.kind === "property") {
        const value = descriptor.value;
        id =
          value.value_state === "value" && typeof value.value === "string"
            ? propertyGroupId(value.property_id, value.value)
            : propertyGroupId(value.property_id, null);
      }
      const pages = grouped.get(id) ?? [];
      pages.push(page);
      grouped.set(id, pages);
    }
    return Object.fromEntries(
      Array.from(grouped, ([id, pages]) => [
        id,
        {
          total: pages.reduce((sum, page) => sum + page.total, 0),
          loaded: pages.reduce((sum, page) => sum + page.loaded, 0),
          hasMore: pages.some((page) => page.hasMore),
          isLoading: pages.some((page) => page.isLoading),
          isFetching: pages.some((page) => page.isFetching),
          isError: pages.some((page) => page.isError),
          loadMore: () => {
            for (const page of pages) {
              if (page.hasMore) page.loadMore();
            }
          },
          retry: () => {
            for (const page of pages) {
              if (page.isError) page.retry();
            }
          },
        },
      ]),
    ) as Record<string, IssueGroupPageState>;
  }, [groupBranches]);
  const groups = useMemo(
    () => {
      const built =
        hydratedExecutorGroups ??
        hydratedProjectGroups ??
        buildGroups(issues, visibleStatuses, grouping, {
          getActorName,
          groupingProperty,
          projectMap,
          noExecutorLabel: t(($) => $.filters.no_executor),
          noValueLabel: t(($) => $.board.no_value),
          ...projectColumnLabels,
        });
      return built.map((group) => ({
        ...group,
        totalCount: groupPagination?.[group.id]?.total ?? group.totalCount,
      }));
    },
    [hydratedExecutorGroups, hydratedProjectGroups, issues, visibleStatuses, grouping, getActorName, groupingProperty, projectMap, projectColumnLabels, groupPagination, t],
  );
  // Empty status columns remain server-backed drop targets, but they do not
  // earn a full 280px board column. Keep them in the local drag map and move
  // them into the hidden panel until a card is placed there.
  const emptyStatusIds = useMemo(() => {
    if (grouping !== "status") return new Set<string>();
    const ids = new Set<string>();
    for (const group of groups) {
      if (!group.status) continue;
      const page = statusPagination?.[group.status];
      const hasLoadedIssue = groupedIssues.some((issue) =>
        issueMatchesGroup(issue, group),
      );
      if (
        page &&
        !page.isLoading &&
        !page.isFetching &&
        !page.isError &&
        page.total === 0 &&
        !hasLoadedIssue
      ) {
        ids.add(group.id);
      }
    }
    return ids;
  }, [groupedIssues, groups, grouping, statusPagination]);
  // A dropped card can take one render to appear in the server-backed status
  // branch. Keep an auto-hidden target expanded during that handoff so the
  // card does not appear to disappear after a successful drop.
  const [revealedEmptyStatuses, setRevealedEmptyStatuses] = useState<
    Set<IssueStatusCategory>
  >(() => new Set<IssueStatusCategory>());

  const renderedGroups = useMemo(
    () =>
      groups.filter(
        (group) =>
          !emptyStatusIds.has(group.id) ||
          (group.status !== undefined &&
            revealedEmptyStatuses.has(group.status)),
      ),
    [emptyStatusIds, groups, revealedEmptyStatuses],
  );
  const hiddenBoardStatuses = useMemo(() => {
    const statuses = [...hiddenStatuses];
    const seen = new Set(statuses);
    for (const group of groups) {
      if (
        group.status &&
        emptyStatusIds.has(group.id) &&
        !revealedEmptyStatuses.has(group.status) &&
        !seen.has(group.status)
      ) {
        statuses.push(group.status);
        seen.add(group.status);
      }
    }
    return statuses.filter((status) => !revealedEmptyStatuses.has(status));
  }, [emptyStatusIds, groups, hiddenStatuses, revealedEmptyStatuses]);
  const hiddenDropStatusSet = useMemo(
    () => new Set(droppableHiddenStatuses ?? hiddenStatuses),
    [droppableHiddenStatuses, hiddenStatuses],
  );
  const railStatuses = useMemo(() => grouping === "status" ? hiddenBoardStatuses.filter((status) =>
    hiddenDropStatusSet.has(status) || emptyStatusIds.has(statusGroupId(status)),
  ) : [], [grouping, hiddenBoardStatuses, hiddenDropStatusSet, emptyStatusIds]);
  const boardGroups = useMemo(() => {
    const byID = new Map(renderedGroups.map((group) => [group.id, group]));
    for (const status of railStatuses) {
      const group = makeStatusGroup(status);
      if (!byID.has(group.id)) byID.set(group.id, { ...group, totalCount: statusPagination?.[status]?.total ?? 0 });
    }
    return [...byID.values()];
  }, [renderedGroups, railStatuses, statusPagination]);
  const hiddenDropStatuses = useMemo(() => {
    const statuses = hiddenStatuses.filter((status) =>
      hiddenDropStatusSet.has(status),
    );
    for (const group of groups) {
      if (
        group.status &&
        emptyStatusIds.has(group.id) &&
        !revealedEmptyStatuses.has(group.status) &&
        !statuses.includes(group.status)
      ) {
        statuses.push(group.status);
      }
    }
    return statuses.filter((status) => !revealedEmptyStatuses.has(status));
  }, [
    emptyStatusIds,
    groups,
    hiddenDropStatusSet,
    hiddenStatuses,
    revealedEmptyStatuses,
  ]);
  const dropGroups = useMemo(() => {
    if (grouping !== "status") return groups;
    const result: BoardColumnGroup[] = [...groups];
    const seen = new Set(result.map((group) => group.id));
    for (const status of hiddenDropStatuses) {
      const group = makeStatusGroup(status);
      if (seen.has(group.id)) continue;
      result.push(group);
      seen.add(group.id);
    }
    return result;
  }, [grouping, groups, hiddenDropStatuses]);
  const groupIds = useMemo(
    () => new Set(dropGroups.map((group) => group.id)),
    [dropGroups],
  );
  const groupMap = useMemo(
    () => new Map(dropGroups.map((group) => [group.id, group])),
    [dropGroups],
  );
  const collisionDetection = useMemo<CollisionDetection>(() => {
    const itemCollision = makeKanbanCollision(groupIds);
    return (args) => groupIds.has(String(args.active.id))
      ? closestCenter({ ...args, droppableContainers: args.droppableContainers.filter((column) => groupIds.has(String(column.id))) })
      : itemCollision(args);
  }, [groupIds]);

  // --- Drag state ---
  useEffect(() => {
    setRevealedEmptyStatuses((previous) => {
      let changed = false;
      const next = new Set(previous);
      for (const status of previous) {
        if (!emptyStatusIds.has(statusGroupId(status))) {
          next.delete(status);
          changed = true;
        }
      }
      return changed ? next : previous;
    });
  }, [emptyStatusIds]);

  const showHiddenStatus = useCallback(
    (status: IssueStatusCategory) => {
      if (emptyStatusIds.has(statusGroupId(status))) {
        setRevealedEmptyStatuses((previous) => {
          if (previous.has(status)) return previous;
          return new Set(previous).add(status);
        });
      }
      viewStoreApi.getState().showStatus(status);
    },
    [emptyStatusIds, viewStoreApi],
  );

  // Shared drag/settle primitive: owns the local column mirror, the
  // dragging/settling locks, the post-move animation-frame throttle, and the
  // settle callback. Shared with list-view (and swimlane) so the surfaces
  // can't drift apart. Local columns follow TQ between drags via the resync
  // effect below; during a drag/settle they are frozen by the locks.
  const {
    columns,
    setColumns,
    columnsRef,
    isDraggingRef,
    isSettlingRef,
    recentlyMovedRef,
    settleVersion,
    beginSettle,
  } = useDragSettle(() =>
    buildColumns(groupedIssues, dropGroups, grouping, groupingOptionIds),
  );
  const displayedColumns = useMemo(() => {
    let next = columns;
    for (const group of boardGroups) {
      if (next[group.id]) continue;
      if (next === columns) next = { ...columns };
      next[group.id] = [];
    }
    return next;
  }, [boardGroups, columns]);

  useEffect(() => {
    if (!isDraggingRef.current && !isSettlingRef.current) {
      setColumns((previous) => {
        const refreshed = buildColumns(groupedIssues, dropGroups, grouping, groupingOptionIds);
        return preserveColumnOrder(previous, refreshed);
      });
    }
  }, [
    groupedIssues,
    dropGroups,
    grouping,
    groupingOptionIds,
    settleVersion,
    setColumns,
    isDraggingRef,
    isSettlingRef,
  ]);

  const orderedBoardGroups = useMemo(() => {
    const byID = new Map(boardGroups.map((group) => [group.id, group]));
    return Object.keys(displayedColumns).flatMap((id) => {
      const group = byID.get(id);
      return group ? [group] : [];
    });
  }, [boardGroups, displayedColumns]);

  // --- Issue map ---
  // Frozen during drag so BoardColumn/DraggableBoardCard props stay
  // referentially stable even if a TQ refetch lands mid-drag.
  const issueMap = useMemo(() => {
    const map = new Map<string, Issue>();
    for (const issue of groupedIssues) map.set(issue.id, issue);
    return map;
  }, [groupedIssues]);

  const issueMapRef = useRef(issueMap);
  if (!isDraggingRef.current && !isSettlingRef.current) {
    issueMapRef.current = issueMap;
  }

  // #6700: drag empty board background with the left button to pan horizontally
  // (Trello/Linear). Card drags start on `[data-board-card]` and are ignored.
  const pan = useBoardDragPan<HTMLDivElement>();

  const handleDragStart = useCallback(
    (_event: DragStartEvent) => {
      isDraggingRef.current = true;
    },
    [isDraggingRef],
  );

  const handleDragOver = useCallback(
    (event: DragOverEvent) => {
      const { active, over } = event;
      if (!over || groupIds.has(String(active.id)) || recentlyMovedRef.current) return;

      const activeId = active.id as string;
      const overId = over.id as string;

      setColumns((prev) => {
        const activeCol = findColumn(prev, activeId, groupIds);
        const overCol = findColumn(prev, overId, groupIds);
        if (!activeCol || !overCol || activeCol === overCol) return prev;

        if (sortBy !== "position") return prev;

        recentlyMovedRef.current = true;
        const oldIds = prev[activeCol]!.filter((id) => id !== activeId);
        const newIds = [...(prev[overCol] ?? [])];
        const overIndex = newIds.indexOf(overId);
        const insertIndex = overIndex >= 0 ? overIndex : newIds.length;
        newIds.splice(insertIndex, 0, activeId);
        return { ...prev, [activeCol]: oldIds, [overCol]: newIds };
      });
    },
    [groupIds, sortBy, recentlyMovedRef, setColumns],
  );

  const handleDragEnd = useCallback(
    (event: DragEndEvent) => {
      const { active, over } = event;
      isDraggingRef.current = false;

      const resetColumns = () =>
        setColumns((previous) => preserveColumnOrder(
          previous, buildColumns(groupedIssues, dropGroups, grouping, groupingOptionIds),
        ));

      if (!over) {
        resetColumns();
        return;
      }

      const activeId = active.id as string;
      const overId = over.id as string;

      const cols = columnsRef.current;
      const activeCol = findColumn(cols, activeId, groupIds);
      const overCol = findColumn(cols, overId, groupIds);
      if (!activeCol || !overCol) {
        resetColumns();
        return;
      }

      // Same-column reorder (manual sort only)
      let finalColumns = cols;
      if (activeCol === overCol && sortBy === "position") {
        const ids = cols[activeCol]!;
        const oldIndex = ids.indexOf(activeId);
        const newIndex = ids.indexOf(overId);
        if (oldIndex !== -1 && newIndex !== -1 && oldIndex !== newIndex) {
          const reordered = arrayMove(ids, oldIndex, newIndex);
          finalColumns = { ...cols, [activeCol]: reordered };
          setColumns(finalColumns);
        }
      }

      const finalCol = sortBy === "position"
        ? findColumn(finalColumns, activeId, groupIds)
        : overCol;
      if (!finalCol) {
        resetColumns();
        return;
      }
      const finalGroup = groupMap.get(finalCol);
      if (!finalGroup) {
        resetColumns();
        return;
      }

      const map = issueMapRef.current;
      const revealDroppedStatus = () => {
        const targetStatus = finalGroup.status;
        if (!targetStatus || !hiddenDropStatuses.includes(targetStatus)) return;
        showHiddenStatus(targetStatus);
      };
      const restoreDroppedStatus = () => {
        const targetStatus = finalGroup.status;
        if (!targetStatus || !hiddenDropStatuses.includes(targetStatus)) return;
        if (emptyStatusIds.has(finalGroup.id)) {
          setRevealedEmptyStatuses((previous) => {
            if (!previous.has(targetStatus)) return previous;
            const next = new Set(previous);
            next.delete(targetStatus);
            return next;
          });
        } else {
          viewStoreApi.getState().hideStatus(targetStatus);
        }
      };


      if (sortBy !== "position") {
        // Cross-column: only update group (status/executor), keep original position.
        const currentIssue = map.get(activeId);
        if (!currentIssue || issueMatchesGroup(currentIssue, finalGroup)) {
          resetColumns();
          return;
        }
        // Optimistically move the card into the target column *now*. Without
        // this, the sortBy != "position" path never touches local columns on
        // drop, so onDragOver having been a no-op leaves the card in its origin
        // column for the whole request — it only jumps across when the mutation
        // settles. That is the "snaps back to origin, then moves" glitch.
        // Placement mirrors the cache (insertByPosition) so the settle rebuild
        // from TanStack Query is a visual no-op.
        const targetIds = insertIdByPosition(
          (cols[overCol] ?? []).filter((id) => id !== activeId),
          activeId,
          currentIssue.position,
          map,
        );
        setColumns((prev) => {
          const fromIds = (prev[activeCol] ?? []).filter((cid) => cid !== activeId);
          return { ...prev, [activeCol]: fromIds, [overCol]: targetIds };
        });
        const committed = onMoveIssue(
          activeId,
          {
            ...getMoveUpdates(finalGroup, currentIssue.position, currentIssue),
            ...getMoveAnchors(targetIds, activeId),
          },
          { onSettled: beginSettle(), onSuccess: revealDroppedStatus, onError: restoreDroppedStatus },
        );
        if (committed === false) {
          resetColumns();
          return;
        }
        revealDroppedStatus();
        applyPropertyGroupValue(finalGroup, activeId);
        return;
      }

      const finalIds = finalColumns[finalCol]!;
      const newPosition = computePosition(finalIds, activeId, map);
      const currentIssue = map.get(activeId);

      if (
        currentIssue &&
        issueMatchesGroup(currentIssue, finalGroup) &&
        currentIssue.position === newPosition
      ) {
        return;
      }

      // beginSettle() holds the lock and returns the onSettled callback that
      // releases it and resyncs local columns from the cache: a no-op on
      // success (onSuccess already patched the moved card in place), the revert
      // on error (onError restored the snapshot). Without it a failed move would
      // strand the card at the drop target, since onSettled no longer refetches.
      const committed = onMoveIssue(
        activeId,
        {
          ...getMoveUpdates(finalGroup, newPosition, currentIssue),
          ...getMoveAnchors(finalIds, activeId),
        },
        { onSettled: beginSettle(), onSuccess: revealDroppedStatus, onError: restoreDroppedStatus },
      );
      if (committed === false) {
        resetColumns();
        return;
      }
      revealDroppedStatus();
      applyPropertyGroupValue(finalGroup, activeId);
    },
    [groupedIssues, dropGroups, grouping, groupingOptionIds, onMoveIssue, groupIds, groupMap, sortBy, beginSettle, columnsRef, isDraggingRef, setColumns, applyPropertyGroupValue, emptyStatusIds, hiddenDropStatuses, showHiddenStatus, viewStoreApi],
  );

  // An aborted drag (pointercancel, window resize, tab hide, Escape) fires
  // onDragCancel instead of onDragEnd. Releasing the drag lock here keeps the
  // column mirror resyncing with the cache afterwards — see the same handler in
  // list-view for the touch path that makes this routine (MUL-6240).
  const handleDragCancel = useCallback(() => {
    isDraggingRef.current = false;
    setColumns((previous) => preserveColumnOrder(
      previous, buildColumns(groupedIssues, dropGroups, grouping, groupingOptionIds),
    ));
  }, [groupedIssues, dropGroups, grouping, groupingOptionIds, setColumns, isDraggingRef]);

  const handleMove = useCallback(
    ({ event }: KanbanMoveEvent) => handleDragEnd(event),
    [handleDragEnd],
  );

  // ReUI invokes `onDragEnd` before `onMove`, but it never invokes `onMove`
  // when the pointer is released outside every droppable. Release the shared
  // drag lock on that path here; successful drops continue through handleMove
  // exactly once.
  const handleKanbanDragEnd = useCallback(
    (event: DragEndEvent) => {
      if (groupIds.has(String(event.active.id))) {
        isDraggingRef.current = false;
        return;
      }
      if (!event.over) handleDragEnd(event);
    },
    [handleDragEnd, groupIds, isDraggingRef],
  );


  return (
    <div className="bg-muted flex min-h-0 min-w-0 w-full flex-1 items-stretch overflow-hidden px-3 py-2">
    <Kanban
      value={displayedColumns}
      onValueChange={setColumns}
      getItemValue={issueId}
      onMove={handleMove}
      collisionDetection={collisionDetection}
      onDragStart={handleDragStart}
      onDragOver={handleDragOver}
      onDragEnd={handleKanbanDragEnd}
      onDragCancel={handleDragCancel}
      className="flex min-h-0 min-w-0 w-full flex-1 flex-col overflow-hidden"
    >
      <BoardScrollArea
        ref={pan.ref}
        onPointerDown={pan.onPointerDown}
        onPointerMove={pan.onPointerMove}
        onPointerUp={pan.onPointerUp}
        onPointerCancel={pan.onPointerCancel}
        onLostPointerCapture={pan.onLostPointerCapture}
      >
        <SortableContext items={Object.keys(displayedColumns)} strategy={horizontalListSortingStrategy}>
        <div data-slot="kanban-board" className="flex h-full min-h-full min-w-full items-stretch gap-3 p-1">
        {boardGroups.length === 0 ? (
          groupBranches?.isError ? (
            <button
              type="button"
              className="flex min-w-full flex-1 items-center justify-center text-body text-destructive hover:underline"
              onClick={groupBranches.retryGroups}
            >
              {t(($) => $.table.load_more_failed_retry)}
            </button>
          ) : (
            <div className="flex min-w-full flex-1 items-center justify-center text-body text-muted-foreground">
              {t(($) => $.board.empty_grouping)}
            </div>
          )
        ) : (
          orderedBoardGroups.map((group) =>
            isStatusGroup(group) ? (
              <ServerPaginatedBoardColumn
                key={group.id}
                group={group}
                issueIds={displayedColumns[group.id] ?? EMPTY_IDS}
                issueMap={issueMapRef.current}
                childProgressMap={childProgressMap}
                projectMap={projectMap}
                page={statusPagination?.[group.status]}
                collapsed={railStatuses.includes(group.status)}
                onExpand={() => showHiddenStatus(group.status)}
                projectId={projectId}
                onCreateIssue={onCreateIssue}
                sortLabel={sortLabel}
              />
            ) : (
              groupPagination?.[group.id] ? (
                <ServerPaginatedBoardColumn
                  key={group.id}
                  group={group}
                  issueIds={displayedColumns[group.id] ?? EMPTY_IDS}
                  issueMap={issueMapRef.current}
                  childProgressMap={childProgressMap}
                  projectMap={projectMap}
                  page={groupPagination[group.id]!}
                  projectId={projectId}
                  onCreateIssue={onCreateIssue}
                  sortLabel={sortLabel}
                />
              ) : (
                <BoardColumn
                  key={group.id}
                  group={group}
                  issueIds={displayedColumns[group.id] ?? EMPTY_IDS}
                  issueMap={issueMapRef.current}
                  childProgressMap={childProgressMap}
                  projectMap={projectMap}
                  projectId={projectId}
                  onCreateIssue={onCreateIssue}
                  totalCount={group.totalCount}
                  sortLabel={sortLabel}
                />
              )
            ),
          )
        )}
        {groupBranches?.hasMoreGroups && (
          <div className="flex w-8 shrink-0 items-center justify-center">
            <InfiniteScrollSentinel
              onVisible={groupBranches.loadMoreGroups}
              loading={groupBranches.isLoadingMoreGroups}
            />
          </div>
        )}
        </div>
        </SortableContext>
      </BoardScrollArea>

      <KanbanOverlay>
        {({ value, variant }) => {
          if (variant === "column") {
            const group = orderedBoardGroups.find((column) => column.id === String(value));
            if (!group) return null;
            return (
              <BoardColumn
                group={group}
                issueIds={displayedColumns[group.id] ?? EMPTY_IDS}
                issueMap={issueMapRef.current}
                childProgressMap={childProgressMap}
                projectMap={projectMap}
                isOverlay
              />
            );
          }
          const activeIssue = issueMapRef.current.get(String(value));
          if (!activeIssue) return null;
          return (
            <div style={{ width: BOARD_CARD_WIDTH }} className="shadow-lg">
              <BoardCardContent
                issue={activeIssue}
                childProgress={childProgressMap.get(activeIssue.id)}
                project={
                  activeIssue.project_id
                    ? projectMap?.get(activeIssue.project_id)
                    : undefined
                }
              />
            </div>
          );
        }}
      </KanbanOverlay>
    </Kanban>
    </div>
  );
}


const ServerPaginatedBoardColumn = memo(function ServerPaginatedBoardColumn({
  group,
  issueIds,
  issueMap,
  childProgressMap,
  projectMap,
  page,
  projectId,
  onCreateIssue,
  sortLabel,
  collapsed,
  onExpand,
}: {
  group: BoardColumnGroup;
  issueIds: string[];
  issueMap: Map<string, Issue>;
  childProgressMap?: Map<string, ChildProgress>;
  projectMap?: Map<string, Project>;
  page?: IssueStatusPageState | IssueGroupPageState;
  projectId?: string;
  onCreateIssue?: (defaults: IssueCreateDefaults) => void;
  sortLabel?: string | null;
  collapsed?: boolean;
  onExpand?: () => void;
}) {
  const footer = page ? (
    <ListLoadMoreFooter
      hasMore={page.hasMore}
      isLoading={page.isLoading || page.isFetching}
      total={page.total}
      onLoadMore={page.loadMore}
      isError={page.isError}
      onRetry={page.retry}
    />
  ) : undefined;
  return (
    <BoardColumn
      group={group}
      issueIds={issueIds}
      issueMap={issueMap}
      childProgressMap={childProgressMap}
      projectMap={projectMap}
      totalCount={page?.total ?? group.totalCount}
      projectId={projectId}
      onCreateIssue={onCreateIssue}
      sortLabel={sortLabel}
      collapsed={collapsed}
      onExpand={onExpand}
      footer={footer}
    />
  );
});


/**
 * Memoized: the surface controller re-renders on loading-flag flips (e.g. a
 * query enabling when the view changes) — without memo every such flip
 * re-rendered this entire view tree (hundreds of ms). All props are
 * referentially stable useMemo/useCallback outputs from the controller.
 */
export const BoardView = memo(BoardViewImpl);
