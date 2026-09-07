/**
 * Copy for the mobile Dependency Graph screen.
 *
 * The screen shipped with every string inlined in English, including the
 * readiness states it renders next to every task. Interpolated strings are
 * functions rather than templates with placeholders so each locale controls
 * its own word order and counters.
 */
import {
  normalizeProductLocale,
  PRODUCT_LOCALES,
  type ProductLocale,
} from "./locale";

export type TaskGraphCopy = {
  title: string;
  back: string;
  loadFailed: (reason: string) => string;
  unknownError: string;
  retry: string;
  emptyTitle: string;
  emptyBody: string;
  activePlans: (count: number) => string;
  totals: (totals: {
    total: number;
    ready: number;
    running: number;
    blocked: number;
  }) => string;
  filterAll: string;
  filterReady: string;
  filterRunning: string;
  filterBlocked: string;
  planLabel: (shortId: string) => string;
  planFallbackGoal: string;
  attentionRequired: (reason: string) => string;
  attentionFallbackReason: string;
  wave: (index: number) => string;
  openNode: (identifier: string) => string;
  gateOpen: string;
  gateBlocked: string;
  prerequisites: (satisfied: number, total: number) => string;
  dependencies: string;
  edgeSatisfied: string;
  edgeBlocked: string;
  noMatches: string;
  stateReady: string;
  stateRunning: string;
  stateBlocked: string;
  stateDone: string;
  stateCancelled: string;
  stateTodo: string;
};

const COPY: Record<ProductLocale, TaskGraphCopy> = {
  en: {
    title: "Dependency Graph",
    back: "Back",
    loadFailed: (reason) => `Failed to load dependency graphs: ${reason}`,
    unknownError: "unknown error",
    retry: "Retry",
    emptyTitle: "No dependency graphs yet",
    emptyBody:
      "Apply a dependency graph to a parent task to see its execution plan here.",
    activePlans: (count) => `${count} active plans`,
    totals: ({ total, ready, running, blocked }) =>
      `${total} tasks · ${ready} ready · ${running} running · ${blocked} blocked`,
    filterAll: "All",
    filterReady: "Ready",
    filterRunning: "Running",
    filterBlocked: "Blocked",
    planLabel: (shortId) => `Plan · ${shortId}`,
    planFallbackGoal: "Dependency graph execution plan",
    attentionRequired: (reason) => `Planner attention required: ${reason}`,
    attentionFallbackReason: "review the execution gate",
    wave: (index) => `Wave ${index}`,
    openNode: (identifier) => `Open ${identifier}`,
    gateOpen: "Gate open",
    gateBlocked: "Gate blocked",
    prerequisites: (satisfied, total) =>
      `${satisfied}/${total} prerequisites satisfied`,
    dependencies: "Dependencies",
    edgeSatisfied: "Satisfied",
    edgeBlocked: "Blocked",
    noMatches: "No tasks match this filter.",
    stateReady: "Ready",
    stateRunning: "Running",
    stateBlocked: "Blocked",
    stateDone: "Done",
    stateCancelled: "Cancelled",
    stateTodo: "Todo",
  },
  "zh-Hans": {
    title: "依赖图",
    back: "返回",
    loadFailed: (reason) => `加载依赖图失败：${reason}`,
    unknownError: "未知错误",
    retry: "重试",
    emptyTitle: "还没有依赖图",
    emptyBody: "为父任务应用依赖图后，即可在这里查看它的执行计划。",
    activePlans: (count) => `${count} 个进行中的计划`,
    totals: ({ total, ready, running, blocked }) =>
      `${total} 个任务 · ${ready} 个就绪 · ${running} 个执行中 · ${blocked} 个被阻塞`,
    filterAll: "全部",
    filterReady: "就绪",
    filterRunning: "执行中",
    filterBlocked: "被阻塞",
    planLabel: (shortId) => `计划 · ${shortId}`,
    planFallbackGoal: "依赖图执行计划",
    attentionRequired: (reason) => `规划器需要人工介入：${reason}`,
    attentionFallbackReason: "请检查执行关卡",
    wave: (index) => `第 ${index} 波`,
    openNode: (identifier) => `打开 ${identifier}`,
    gateOpen: "关卡已开放",
    gateBlocked: "关卡被阻塞",
    prerequisites: (satisfied, total) =>
      `已满足 ${satisfied}/${total} 项前置条件`,
    dependencies: "依赖关系",
    edgeSatisfied: "已满足",
    edgeBlocked: "被阻塞",
    noMatches: "没有任务符合该筛选条件。",
    stateReady: "就绪",
    stateRunning: "执行中",
    stateBlocked: "被阻塞",
    stateDone: "已完成",
    stateCancelled: "已取消",
    stateTodo: "待办",
  },
};

export function getTaskGraphCopy(
  language: string | null | undefined,
): TaskGraphCopy {
  return COPY[normalizeProductLocale(language)];
}

/**
 * Readiness state comes from the server as an open string: the four known
 * states plus whatever a workspace's own catalog defines. An unrecognized
 * state is returned verbatim rather than forced into one of the known
 * labels — showing the raw key is honest, mislabelling it is not.
 */
export function getTaskGraphStateLabel(
  copy: TaskGraphCopy,
  state: string,
): string {
  switch (state) {
    case "ready":
      return copy.stateReady;
    case "running":
      return copy.stateRunning;
    case "blocked":
      return copy.stateBlocked;
    case "done":
      return copy.stateDone;
    case "cancelled":
      return copy.stateCancelled;
    case "todo":
      return copy.stateTodo;
    default:
      return state || copy.stateTodo;
  }
}

export const TASK_GRAPH_COPY_LOCALES = PRODUCT_LOCALES;
