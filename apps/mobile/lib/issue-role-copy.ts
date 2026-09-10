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
  reviewSubmissionDescription: string;
  reviewWorktree: string;
  reviewWorktreePlaceholder: string;
  reviewBranch: string;
  reviewBranchPlaceholder: string;
  reviewCommit: string;
  reviewCommitPlaceholder: string;
  reviewPullRequests: string;
  reviewPullRequestsPlaceholder: string;
  reviewPullRequestsHint: string;
  submitReview: string;
  submittingReview: string;
  invalidReviewEvidence: string;
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
    reviewSubmissionDescription:
      "Attach the exact worktree, branch, full commit SHA, and pull request used for this review.",
    reviewWorktree: "Worktree",
    reviewWorktreePlaceholder: "/path/to/worktree",
    reviewBranch: "Branch",
    reviewBranchPlaceholder: "codex/issue-42",
    reviewCommit: "Commit",
    reviewCommitPlaceholder: "Full 40 or 64 character SHA",
    reviewPullRequests: "Pull requests",
    reviewPullRequestsPlaceholder: "https://github.com/org/repo/pull/42",
    reviewPullRequestsHint: "Enter one pull request URL per line.",
    submitReview: "Submit for review",
    submittingReview: "Submitting…",
    invalidReviewEvidence:
      "Enter a worktree, branch, full commit SHA, and at least one valid pull request URL.",
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
    reviewSubmissionDescription:
      "填写本次审核对应的 worktree、分支、完整 commit SHA 和 Pull Request。",
    reviewWorktree: "Worktree",
    reviewWorktreePlaceholder: "/path/to/worktree",
    reviewBranch: "分支",
    reviewBranchPlaceholder: "codex/issue-42",
    reviewCommit: "Commit",
    reviewCommitPlaceholder: "完整的 40 或 64 位 SHA",
    reviewPullRequests: "Pull Request",
    reviewPullRequestsPlaceholder: "https://github.com/org/repo/pull/42",
    reviewPullRequestsHint: "每行填写一个 Pull Request URL。",
    submitReview: "提交审核",
    submittingReview: "正在提交…",
    invalidReviewEvidence:
      "请填写 worktree、分支、完整 commit SHA，以及至少一个有效的 Pull Request URL。",
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
