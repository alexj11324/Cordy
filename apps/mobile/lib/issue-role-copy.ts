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
    updateFailed: "이슈를 업데이트하지 못했습니다",
  },
};

export function getIssueRoleCopy(
  language: string | null | undefined,
): IssueRoleCopy {
  return COPY[normalizeProductLocale(language)];
}

export const ISSUE_ROLE_COPY_LOCALES = PRODUCT_LOCALES;
