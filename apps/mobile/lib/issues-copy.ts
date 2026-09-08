import type { IssuePriority } from "@orvilo/core/types";
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
};

export function getIssuesCopy(
  language: string | null | undefined,
): IssuesCopy {
  return COPY[normalizeProductLocale(language)];
}
