import type { IssuePriority } from "@patchbay/core/types";
import {
  normalizeProductLocale,
  type ProductLocale,
} from "@/lib/locale";

export type IssuesCopy = {
  title: string;
  myTitle: string;
  back: string;
  filter: string;
  retry: string;
  unknownError: string;
  loadFailed: (reason: string) => string;
  filteredEmpty: string;
  scopes: {
    all: string;
    members: string;
    agents: string;
    owned: string;
    created: string;
  };
  empty: {
    workspace: string;
    memberOwner: string;
    agentExecutor: string;
    owned: string;
    created: string;
    agents: string;
  };
  priority: Record<IssuePriority, string>;
};

const COPY: Record<ProductLocale, IssuesCopy> = {
  en: {
    title: "Issues",
    myTitle: "My Issues",
    back: "Back",
    filter: "Filter",
    retry: "Retry",
    unknownError: "unknown error",
    loadFailed: (reason) => `Failed to load issues: ${reason}`,
    filteredEmpty: "No issues match the current filters.",
    scopes: {
      all: "All",
      members: "Members",
      agents: "Agents",
      owned: "Owned",
      created: "Created",
    },
    empty: {
      workspace: "No issues in this workspace.",
      memberOwner: "No issues have a member as owner.",
      agentExecutor: "No issues have an agent or team as executor.",
      owned: "No issues owned by you.",
      created: "You haven't created any issues.",
      agents: "No issues have your agents or teams as executor yet.",
    },
    priority: {
      none: "No priority",
      low: "Low",
      medium: "Medium",
      high: "High",
      urgent: "Urgent",
    },
  },
  "zh-Hans": {
    title: "问题",
    myTitle: "我的问题",
    back: "返回",
    filter: "筛选",
    retry: "重试",
    unknownError: "未知错误",
    loadFailed: (reason) => `加载问题失败：${reason}`,
    filteredEmpty: "没有符合当前筛选条件的问题。",
    scopes: {
      all: "全部",
      members: "成员",
      agents: "Agent",
      owned: "我负责的",
      created: "我创建的",
    },
    empty: {
      workspace: "此工作区还没有问题。",
      memberOwner: "没有成员作为所有者的问题。",
      agentExecutor: "没有 Agent 或团队作为执行者的问题。",
      owned: "没有由你负责的问题。",
      created: "你还没有创建问题。",
      agents: "还没有以你的 Agent 或团队为执行者的问题。",
    },
    priority: {
      none: "无优先级",
      low: "低",
      medium: "中",
      high: "高",
      urgent: "紧急",
    },
  },
  ja: {
    title: "Issue",
    myTitle: "自分のIssue",
    back: "戻る",
    filter: "フィルター",
    retry: "再試行",
    unknownError: "不明なエラー",
    loadFailed: (reason) => `Issueの読み込みに失敗しました：${reason}`,
    filteredEmpty: "現在のフィルターに一致するIssueはありません。",
    scopes: {
      all: "すべて",
      members: "メンバー",
      agents: "エージェント",
      owned: "担当",
      created: "作成済み",
    },
    empty: {
      workspace: "このワークスペースにIssueはありません。",
      memberOwner: "メンバーがオーナーのIssueはありません。",
      agentExecutor: "エージェントまたはチームが実行者のIssueはありません。",
      owned: "自分が担当するIssueはありません。",
      created: "作成したIssueはまだありません。",
      agents: "自分のエージェントまたはチームが実行者のIssueはまだありません。",
    },
    priority: {
      none: "優先度なし",
      low: "低",
      medium: "中",
      high: "高",
      urgent: "緊急",
    },
  },
  ko: {
    title: "이슈",
    myTitle: "내 이슈",
    back: "뒤로",
    filter: "필터",
    retry: "다시 시도",
    unknownError: "알 수 없는 오류",
    loadFailed: (reason) => `이슈를 불러오지 못했습니다: ${reason}`,
    filteredEmpty: "현재 필터와 일치하는 이슈가 없습니다.",
    scopes: {
      all: "전체",
      members: "멤버",
      agents: "에이전트",
      owned: "내 담당",
      created: "내가 생성",
    },
    empty: {
      workspace: "이 워크스페이스에는 이슈가 없습니다.",
      memberOwner: "멤버가 소유자인 이슈가 없습니다.",
      agentExecutor: "에이전트 또는 팀이 실행자인 이슈가 없습니다.",
      owned: "내가 담당한 이슈가 없습니다.",
      created: "아직 생성한 이슈가 없습니다.",
      agents: "내 에이전트 또는 팀이 실행자인 이슈가 아직 없습니다.",
    },
    priority: {
      none: "우선순위 없음",
      low: "낮음",
      medium: "중간",
      high: "높음",
      urgent: "긴급",
    },
  },
};

export function getIssuesCopy(
  language: string | null | undefined,
): IssuesCopy {
  return COPY[normalizeProductLocale(language)];
}
