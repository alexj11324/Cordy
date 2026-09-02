import {
  normalizeProductLocale,
  PRODUCT_LOCALES,
  type ProductLocale,
} from "./locale";

export type IssueRoleCopy = {
  owner: string;
  executor: string;
  reviewer: string;
  unknown: string;
  unassigned: string;
  searchMembers: string;
  searchExecutors: string;
  searchReviewers: string;
  agent: string;
  team: string;
  needsRuntime: string;
  leaderNeedsRuntime: string;
  noMatches: string;
};

const COPY: Record<ProductLocale, IssueRoleCopy> = {
  en: {
    owner: "Owner",
    executor: "Executor",
    reviewer: "Reviewer",
    unknown: "Unknown",
    unassigned: "Unassigned",
    searchMembers: "Search members",
    searchExecutors: "Search agents and teams",
    searchReviewers: "Search reviewers",
    agent: "Agent",
    team: "Team",
    needsRuntime: "Needs runtime",
    leaderNeedsRuntime: "Leader needs runtime",
    noMatches: "No matches.",
  },
  "zh-Hans": {
    owner: "负责人",
    executor: "执行者",
    reviewer: "审核者",
    unknown: "未知",
    unassigned: "未分配",
    searchMembers: "搜索成员",
    searchExecutors: "搜索 Agent 和团队",
    searchReviewers: "搜索审核者",
    agent: "Agent",
    team: "团队",
    needsRuntime: "需要运行时",
    leaderNeedsRuntime: "负责人需要运行时",
    noMatches: "没有匹配结果。",
  },
  ja: {
    owner: "オーナー",
    executor: "実行者",
    reviewer: "レビュー担当",
    unknown: "不明",
    unassigned: "未割り当て",
    searchMembers: "メンバーを検索",
    searchExecutors: "Agent とチームを検索",
    searchReviewers: "レビュー担当を検索",
    agent: "Agent",
    team: "チーム",
    needsRuntime: "ランタイムが必要",
    leaderNeedsRuntime: "リーダーにランタイムが必要",
    noMatches: "一致する項目はありません。",
  },
  ko: {
    owner: "담당자",
    executor: "실행자",
    reviewer: "검토자",
    unknown: "알 수 없음",
    unassigned: "미할당",
    searchMembers: "멤버 검색",
    searchExecutors: "Agent 및 팀 검색",
    searchReviewers: "검토자 검색",
    agent: "Agent",
    team: "팀",
    needsRuntime: "런타임 필요",
    leaderNeedsRuntime: "리더 런타임 필요",
    noMatches: "일치하는 항목이 없습니다.",
  },
};

export function getIssueRoleCopy(
  language: string | null | undefined,
): IssueRoleCopy {
  return COPY[normalizeProductLocale(language)];
}

export const ISSUE_ROLE_COPY_LOCALES = PRODUCT_LOCALES;
