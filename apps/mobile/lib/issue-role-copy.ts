import {
  normalizeProductLocale,
  PRODUCT_LOCALES,
  type ProductLocale,
} from "./locale";

export type IssueRoleCopy = {
  owner: string;
  executor: string;
  reviewer: string;
  reviewHandoff: string;
  unknown: string;
  unknownOwner: string;
  unknownExecutor: string;
  unknownReviewer: string;
  unassigned: string;
  searchMembers: string;
  searchExecutors: string;
  searchReviewers: string;
  agent: string;
  team: string;
  needsRuntime: string;
  leaderNeedsRuntime: string;
  noMatches: string;
  executorRequired: string;
  reviewerRequired: string;
  reviewerMustDiffer: string;
  reviewerAssignedTo: string;
  reviewerChangedFromTo: string;
  reviewerRemoved: string;
  reviewHandoffFromTo: string;
  reviewRequested: string;
  reviewRequestedFor: string;
  roleAssignments: string;
  roleAssignmentsDescription: string;
  loadIssueFailed: string;
  retry: string;
  updateFailed: string;
};

const COPY: Record<ProductLocale, IssueRoleCopy> = {
  en: {
    owner: "Owner",
    executor: "Executor",
    reviewer: "Reviewer",
    reviewHandoff: "Review handoff",
    unknown: "Unknown",
    unknownOwner: "Unknown owner",
    unknownExecutor: "Unknown executor",
    unknownReviewer: "Unknown reviewer",
    unassigned: "Unassigned",
    searchMembers: "Search members",
    searchExecutors: "Search agents and teams",
    searchReviewers: "Search reviewers",
    agent: "Agent",
    team: "Team",
    needsRuntime: "Needs runtime",
    leaderNeedsRuntime: "Leader needs runtime",
    noMatches: "No matches.",
    executorRequired: "Choose an executor for an issue with work underway.",
    reviewerRequired: "Choose a reviewer before moving this issue into review.",
    reviewerMustDiffer: "The reviewer must be different from the executor.",
    reviewerAssignedTo: "assigned reviewer to {{name}}",
    reviewerChangedFromTo: "changed reviewer from {{from}} to {{to}}",
    reviewerRemoved: "removed reviewer",
    reviewHandoffFromTo: "handed review from {{from}} to {{to}}",
    reviewRequested: "Review requested",
    reviewRequestedFor: "Review requested for {{name}}",
    roleAssignments: "Role assignments",
    roleAssignmentsDescription:
      "When you become or stop being an issue owner, executor, or reviewer.",
    loadIssueFailed: "Failed to load issue",
    retry: "Retry",
    updateFailed: "Failed to update issue",
  },
  "zh-Hans": {
    owner: "负责人",
    executor: "执行者",
    reviewer: "审核者",
    reviewHandoff: "移交审核",
    unknown: "未知",
    unknownOwner: "未知负责人",
    unknownExecutor: "未知执行者",
    unknownReviewer: "未知审核者",
    unassigned: "未分配",
    searchMembers: "搜索成员",
    searchExecutors: "搜索 Agent 和团队",
    searchReviewers: "搜索审核者",
    agent: "Agent",
    team: "团队",
    needsRuntime: "需要运行时",
    leaderNeedsRuntime: "负责人需要运行时",
    noMatches: "没有匹配结果。",
    executorRequired: "进行中的任务需要选择执行者。",
    reviewerRequired: "将任务移入审核前，请选择审核者。",
    reviewerMustDiffer: "审核者必须与执行者不同。",
    reviewerAssignedTo: "将审核者设为 {{name}}",
    reviewerChangedFromTo: "将审核者从 {{from}} 改为 {{to}}",
    reviewerRemoved: "移除了审核者",
    reviewHandoffFromTo: "将审核从 {{from}} 移交给 {{to}}",
    reviewRequested: "已请求审核",
    reviewRequestedFor: "已请求{{name}}进行审核",
    roleAssignments: "角色分配",
    roleAssignmentsDescription:
      "当你成为或不再担任任务的负责人、执行者或审核者时。",
    loadIssueFailed: "加载任务失败",
    retry: "重试",
    updateFailed: "更新任务失败",
  },
  ja: {
    owner: "オーナー",
    executor: "実行者",
    reviewer: "レビュー担当",
    reviewHandoff: "レビューを引き継ぐ",
    unknown: "不明",
    unknownOwner: "不明なオーナー",
    unknownExecutor: "不明な実行者",
    unknownReviewer: "不明なレビュー担当",
    unassigned: "未割り当て",
    searchMembers: "メンバーを検索",
    searchExecutors: "Agent とチームを検索",
    searchReviewers: "レビュー担当を検索",
    agent: "Agent",
    team: "チーム",
    needsRuntime: "ランタイムが必要",
    leaderNeedsRuntime: "リーダーにランタイムが必要",
    noMatches: "一致する項目はありません。",
    executorRequired: "進行中の課題には実行者を選択してください。",
    reviewerRequired: "レビューへ移す前にレビュー担当を選択してください。",
    reviewerMustDiffer: "レビュー担当は実行者とは別にしてください。",
    reviewerAssignedTo: "レビュー担当を {{name}} に設定しました",
    reviewerChangedFromTo: "レビュー担当を {{from}} から {{to}} に変更しました",
    reviewerRemoved: "レビュー担当を削除しました",
    reviewHandoffFromTo: "レビューを {{from}} から {{to}} へ引き継ぎました",
    reviewRequested: "レビューを依頼しました",
    reviewRequestedFor: "{{name}} にレビューを依頼しました",
    roleAssignments: "役割の割り当て",
    roleAssignmentsDescription:
      "課題のオーナー、実行者、レビュー担当に設定または解除されたとき。",
    loadIssueFailed: "課題を読み込めませんでした",
    retry: "再試行",
    updateFailed: "課題を更新できませんでした",
  },
  ko: {
    owner: "담당자",
    executor: "실행자",
    reviewer: "검토자",
    reviewHandoff: "검토 인계",
    unknown: "알 수 없음",
    unknownOwner: "알 수 없는 담당자",
    unknownExecutor: "알 수 없는 실행자",
    unknownReviewer: "알 수 없는 검토자",
    unassigned: "미할당",
    searchMembers: "멤버 검색",
    searchExecutors: "Agent 및 팀 검색",
    searchReviewers: "검토자 검색",
    agent: "Agent",
    team: "팀",
    needsRuntime: "런타임 필요",
    leaderNeedsRuntime: "리더 런타임 필요",
    noMatches: "일치하는 항목이 없습니다.",
    executorRequired: "진행 중인 이슈에는 실행자를 선택하세요.",
    reviewerRequired: "검토로 이동하기 전에 검토자를 선택하세요.",
    reviewerMustDiffer: "검토자는 실행자와 달라야 합니다.",
    reviewerAssignedTo: "검토자를 {{name}}(으)로 지정했습니다",
    reviewerChangedFromTo: "검토자를 {{from}}에서 {{to}}(으)로 변경했습니다",
    reviewerRemoved: "검토자를 제거했습니다",
    reviewHandoffFromTo: "검토를 {{from}}에서 {{to}}에게 인계했습니다",
    reviewRequested: "검토를 요청했습니다",
    reviewRequestedFor: "{{name}}에게 검토를 요청했습니다",
    roleAssignments: "역할 할당",
    roleAssignmentsDescription:
      "이슈의 담당자, 실행자 또는 검토자로 지정되거나 해제될 때.",
    loadIssueFailed: "이슈를 불러오지 못했습니다",
    retry: "다시 시도",
    updateFailed: "이슈를 업데이트하지 못했습니다",
  },
};

export function getIssueRoleCopy(
  language: string | null | undefined,
): IssueRoleCopy {
  return COPY[normalizeProductLocale(language)];
}

export const ISSUE_ROLE_COPY_LOCALES = PRODUCT_LOCALES;

export function formatIssueRoleCopy(
  template: string,
  values: Record<string, string>,
): string {
  return template.replace(/\{\{([^}]+)\}\}/g, (match, key: string) =>
    Object.prototype.hasOwnProperty.call(values, key) ? values[key]! : match,
  );
}
