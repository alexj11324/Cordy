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
  ja: {
    title: "依存グラフ",
    back: "戻る",
    loadFailed: (reason) => `依存グラフの読み込みに失敗しました: ${reason}`,
    unknownError: "不明なエラー",
    retry: "再試行",
    emptyTitle: "依存グラフはまだありません",
    emptyBody:
      "親タスクに依存グラフを適用すると、その実行計画がここに表示されます。",
    activePlans: (count) => `進行中の計画 ${count} 件`,
    totals: ({ total, ready, running, blocked }) =>
      `タスク ${total} 件 · 準備完了 ${ready} 件 · 実行中 ${running} 件 · ブロック中 ${blocked} 件`,
    filterAll: "すべて",
    filterReady: "準備完了",
    filterRunning: "実行中",
    filterBlocked: "ブロック中",
    planLabel: (shortId) => `計画 · ${shortId}`,
    planFallbackGoal: "依存グラフの実行計画",
    attentionRequired: (reason) => `プランナーの確認が必要です: ${reason}`,
    attentionFallbackReason: "実行ゲートを確認してください",
    wave: (index) => `ウェーブ ${index}`,
    openNode: (identifier) => `${identifier} を開く`,
    gateOpen: "ゲート開放",
    gateBlocked: "ゲートブロック",
    prerequisites: (satisfied, total) =>
      `前提条件 ${satisfied}/${total} 件を満たしています`,
    dependencies: "依存関係",
    edgeSatisfied: "満たしています",
    edgeBlocked: "ブロック中",
    noMatches: "この条件に一致するタスクはありません。",
    stateReady: "準備完了",
    stateRunning: "実行中",
    stateBlocked: "ブロック中",
    stateDone: "完了",
    stateCancelled: "キャンセル済み",
    stateTodo: "未着手",
  },
  ko: {
    title: "의존성 그래프",
    back: "뒤로",
    loadFailed: (reason) => `의존성 그래프를 불러오지 못했습니다: ${reason}`,
    unknownError: "알 수 없는 오류",
    retry: "다시 시도",
    emptyTitle: "아직 의존성 그래프가 없습니다",
    emptyBody:
      "상위 작업에 의존성 그래프를 적용하면 실행 계획이 여기에 표시됩니다.",
    activePlans: (count) => `진행 중인 계획 ${count}개`,
    totals: ({ total, ready, running, blocked }) =>
      `작업 ${total}개 · 준비 ${ready}개 · 실행 중 ${running}개 · 차단됨 ${blocked}개`,
    filterAll: "전체",
    filterReady: "준비",
    filterRunning: "실행 중",
    filterBlocked: "차단됨",
    planLabel: (shortId) => `계획 · ${shortId}`,
    planFallbackGoal: "의존성 그래프 실행 계획",
    attentionRequired: (reason) => `플래너 확인이 필요합니다: ${reason}`,
    attentionFallbackReason: "실행 게이트를 확인하세요",
    wave: (index) => `웨이브 ${index}`,
    openNode: (identifier) => `${identifier} 열기`,
    gateOpen: "게이트 열림",
    gateBlocked: "게이트 차단됨",
    prerequisites: (satisfied, total) =>
      `선행 조건 ${satisfied}/${total}개 충족`,
    dependencies: "의존 관계",
    edgeSatisfied: "충족됨",
    edgeBlocked: "차단됨",
    noMatches: "이 필터에 해당하는 작업이 없습니다.",
    stateReady: "준비",
    stateRunning: "실행 중",
    stateBlocked: "차단됨",
    stateDone: "완료",
    stateCancelled: "취소됨",
    stateTodo: "할 일",
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
