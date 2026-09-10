import type {
  AgentConversationStarter,
  TaskMessageState,
} from "@orvilo/core/types";
import type { DispatchReasonCopy } from "@/lib/dispatch-reason";

export type ChatLocale = "en" | "zh-Hans";
type ConversationStarter = Pick<AgentConversationStarter, "label" | "prompt">;

export type ChatCopy = {
  chat: string;
  newChat: string;
  sessionsAndAgentPicker: string;
  sessionActions: string;
  chooseAgent: string;
  noAgentsAvailable: string;
  agents: string;
  back: string;
  openChatWith: (name: string) => string;
  needsRuntime: string;
  archived: string;
  noChatsYet: string;
  deleteChatTitle: string;
  deleteChatDescription: (title: string) => string;
  cancel: string;
  delete: string;
  messageNotSent: string;
  permissionAlertTitle: string;
  permissionAlertDescription: string;
  runtimeRequiredTitle: string;
  runtimeRequiredAlertDescription: string;
  noAgentBannerTitle: string;
  noAgentBannerDescription: string;
  noAgentBannerA11y: string;
  runtimeFallbackName: string;
  runtimeRequiredBanner: (name: string) => string;
  offlineFallbackName: string;
  offlineUnstable: (name: string) => string;
  offline: (name: string) => string;
  inputWorking: string;
  inputMessage: string;
  inputUnavailable: string;
  stopAgent: string;
  noAgentSelected: string;
  accessRevoked: string;
  noAgentsWorkspace: string;
  archivedChat: string;
  agentNeedsRuntime: string;
  emptyTitle: (agentName: string | null) => string;
  emptyFirstTimeHint: string;
  fallbackStarters: readonly ConversationStarter[];
  suggestedFollowUps: string;
  showErrorDetails: string;
  showDetails: string;
  noResponse: string;
  repliedIn: (elapsed: string) => string;
  finishedIn: (elapsed: string) => string;
  failedAfter: (elapsed: string) => string;
  processSteps: (count: number) => string;
  queueTitle: (count: number) => string;
  queueFallback: string;
  toolFallback: string;
  toolResultNamed: (tool: string) => string;
  toolState: Readonly<Record<TaskMessageState, string>>;
  toolResultUnnamed: string;
  truncated: string;
  status: {
    retrying: string;
    offline: string;
    reconnecting: string;
    queued: string;
    startingUp: string;
    thinking: string;
    typing: string;
    working: string;
    runningCommand: string;
    readingFiles: string;
    searchingCode: string;
    makingEdits: string;
    searchingWeb: string;
  };
  longPress: {
    copy: string;
    selectText: string;
    cancel: string;
  };
  failure: {
    fallback: string;
    labels: Readonly<Record<string, string>>;
  };
  sendFailure: DispatchReasonCopy;
};

type ChatCopyData = Omit<
  ChatCopy,
  | "deleteChatDescription"
  | "runtimeRequiredBanner"
  | "offlineUnstable"
  | "offline"
  | "emptyTitle"
  | "openChatWith"
  | "repliedIn"
  | "finishedIn"
  | "failedAfter"
  | "processSteps"
  | "queueTitle"
  | "toolResultNamed"
> & {
  deleteChatDescriptionTemplate: string;
  runtimeRequiredBannerTemplate: string;
  offlineUnstableTemplate: string;
  offlineTemplate: string;
  emptyTitleNamed: string;
  emptyTitleDefault: string;
  openChatWithTemplate: string;
  repliedInTemplate: string;
  finishedInTemplate: string;
  failedAfterTemplate: string;
  processStepOne: string;
  processStepsOther: string;
  queueTitleTemplate: string;
  toolResultNamedTemplate: string;
};

const EN_FAILURE_LABELS = {
  queued_expired: "Expired in queue",
  runtime_offline: "Daemon offline",
  runtime_recovery: "Daemon restarted",
  timeout: "Task timed out",
  iteration_limit: "Hit the iteration limit",
  agent_blocked: "Waiting on human input",
  api_invalid_request: "Rejected by the model API",
  skill_bundle_unavailable: "Couldn't download the agent's skills",
  runtime_cli_timeout: "Local runtime CLI timed out",
  "agent_error.provider_auth_or_access": "Provider auth failed",
  "agent_error.provider_quota_limit": "Provider quota exhausted",
  "agent_error.provider_capacity_or_rate_limit": "Rate limited by provider",
  "agent_error.provider_server_error": "Provider server error",
  "agent_error.provider_network": "Network error reaching provider",
  "agent_error.process_failure": "Agent process crashed",
  "agent_error.empty_or_unparseable_output": "Agent returned no usable output",
  "agent_error.agent_timeout": "Agent timed out",
  "agent_error.context_overflow": "Context window exceeded",
  "agent_error.missing_config": "Missing API key or configuration",
  "agent_error.model_not_found_or_unavailable": "Model unavailable",
  "agent_error.runtime_version_unsupported": "Runner CLI version unsupported",
  "agent_error.runtime_missing_executable": "Runner CLI not installed",
  "agent_error.unknown": "Agent execution error",
  agent_error: "Agent execution error",
  codex_semantic_inactivity: "Codex semantic inactivity timeout",
  manual: "Cancelled by user",
} satisfies Readonly<Record<string, string>>;

const ZH_FAILURE_LABELS = {
  queued_expired: "队列中已过期",
  runtime_offline: "运行时离线",
  runtime_recovery: "运行时已重启",
  timeout: "任务超时",
  iteration_limit: "达到迭代上限",
  agent_blocked: "等待人工输入",
  api_invalid_request: "模型 API 拒绝请求",
  skill_bundle_unavailable: "无法下载智能体 skill",
  runtime_cli_timeout: "本地运行时 CLI 超时",
  "agent_error.provider_auth_or_access": "模型服务认证失败",
  "agent_error.provider_quota_limit": "模型服务额度已用尽",
  "agent_error.provider_capacity_or_rate_limit": "模型服务触发限流",
  "agent_error.provider_server_error": "模型服务发生错误",
  "agent_error.provider_network": "连接模型服务失败",
  "agent_error.process_failure": "智能体进程崩溃",
  "agent_error.empty_or_unparseable_output": "智能体未返回可用内容",
  "agent_error.agent_timeout": "智能体运行超时",
  "agent_error.context_overflow": "超出上下文窗口",
  "agent_error.missing_config": "缺少 API 密钥或配置",
  "agent_error.model_not_found_or_unavailable": "模型不可用",
  "agent_error.runtime_version_unsupported": "运行器 CLI 版本不受支持",
  "agent_error.runtime_missing_executable": "未安装运行器 CLI",
  "agent_error.unknown": "智能体运行出错",
  agent_error: "智能体运行出错",
  codex_semantic_inactivity: "Codex 长时间无响应",
  manual: "用户已取消",
} satisfies Readonly<Record<string, string>>;

const EN_STARTERS: readonly ConversationStarter[] = [
  {
    label: "What can you help with?",
    prompt: "What are you best at helping with? Give me a concise overview.",
  },
  {
    label: "Suggest a first task",
    prompt: "Suggest three useful tasks I could delegate to you.",
  },
  {
    label: "Recommend an action",
    prompt:
      "Review what you know about my workspace and recommend a useful first action.",
  },
];

const ZH_STARTERS: readonly ConversationStarter[] = [
  {
    label: "你能帮我做什么？",
    prompt: "你最擅长帮我做什么？请简要介绍。",
  },
  {
    label: "建议第一个任务",
    prompt: "建议三个适合交给你的实用任务。",
  },
  {
    label: "推荐一个操作",
    prompt: "根据你对我的工作区的了解，推荐一个有用的初始操作。",
  },
];

const COPY_DATA = {
  en: {
    chat: "Chat",
    newChat: "New chat",
    sessionsAndAgentPicker: "Sessions and agent picker",
    sessionActions: "Session actions",
    chooseAgent: "Choose an agent",
    noAgentsAvailable: "No agents available.",
    agents: "Agents",
    back: "Back",
    openChatWithTemplate: "Open chat with {name}",
    needsRuntime: "Needs runtime",
    archived: "archived",
    noChatsYet: "No chats yet.",
    deleteChatTitle: "Delete this chat?",
    deleteChatDescriptionTemplate:
      '"{title}" and its messages will be permanently removed. This action cannot be undone.',
    cancel: "Cancel",
    delete: "Delete",
    messageNotSent: "Message not sent",
    permissionAlertTitle: "No permission to run this agent",
    permissionAlertDescription:
      "You no longer have permission to run this agent, so the message was not sent. Ask its owner for access.",
    runtimeRequiredTitle: "Runtime required",
    runtimeRequiredAlertDescription:
      "Bind a runtime to this agent on web or desktop before sending a message.",
    noAgentBannerTitle: "No agents available",
    noAgentBannerDescription:
      "Add or enable an agent in More → Agents to start chatting.",
    noAgentBannerA11y: "No agents available, open agents settings",
    runtimeFallbackName: "This agent",
    runtimeRequiredBannerTemplate:
      "{name} needs a runtime before it can run. Bind one on web or desktop.",
    offlineFallbackName: "This agent",
    offlineUnstableTemplate:
      "{name} may have just disconnected — your message will queue.",
    offlineTemplate:
      "{name} is offline. Messages will wait until its runtime is back.",
    inputWorking: "Agent is working…",
    inputMessage: "Message…",
    inputUnavailable: "Chat unavailable",
    stopAgent: "Stop agent",
    noAgentSelected: "No agent selected",
    accessRevoked: "You can no longer run this agent",
    noAgentsWorkspace: "No agents in this workspace",
    archivedChat: "This chat is archived",
    agentNeedsRuntime: "Agent needs a runtime",
    emptyTitleNamed: "Hi, I'm {name}",
    emptyTitleDefault: "Chat with your agents",
    emptyFirstTimeHint: "Pick an example to start, then edit it before sending.",
    fallbackStarters: EN_STARTERS,
    suggestedFollowUps: "Suggested follow-ups",
    showErrorDetails: "Show error details",
    showDetails: "Show details",
    noResponse: "The agent finished this turn without a text reply.",
    repliedInTemplate: "Replied in {elapsed}",
    finishedInTemplate: "Finished in {elapsed}",
    failedAfterTemplate: "Failed after {elapsed}",
    processStepOne: "1 step",
    processStepsOther: "{count} steps",
    queueTitleTemplate: "{count} queued messages",
    queueFallback: "Queued message",
    toolFallback: "tool",
    toolResultNamedTemplate: "{tool} result: ",
    toolResultUnnamed: "result: ",
    truncated: "(truncated)",
    toolState: {
      "approval-requested": "Awaiting approval",
      "approval-responded": "Approval answered",
      "input-available": "Running",
      "input-streaming": "Pending",
      "output-available": "Completed",
      "output-denied": "Denied",
      "output-error": "Error",
    },
    status: {
      retrying: "Retrying",
      offline: "Offline",
      reconnecting: "Reconnecting",
      queued: "Queued",
      startingUp: "Starting up",
      thinking: "Thinking",
      typing: "Typing",
      working: "Working",
      runningCommand: "Running command",
      readingFiles: "Reading files",
      searchingCode: "Searching code",
      makingEdits: "Making edits",
      searchingWeb: "Searching web",
    },
    longPress: { copy: "Copy", selectText: "Select Text", cancel: "Cancel" },
    failure: { fallback: "Failed", labels: EN_FAILURE_LABELS },
    sendFailure: {
      invocationNotAllowed:
        "You no longer have permission to run this agent, so the message was not sent.",
      runtimeRequired: "Bind a runtime to this agent before sending a message.",
      fallback: "Your message could not be sent. Please try again.",
    },
  },
  "zh-Hans": {
    chat: "聊天",
    newChat: "新对话",
    sessionsAndAgentPicker: "会话与智能体选择",
    sessionActions: "会话操作",
    chooseAgent: "选择智能体",
    noAgentsAvailable: "暂无可用智能体。",
    agents: "智能体",
    back: "返回",
    openChatWithTemplate: "打开与 {name} 的聊天",
    needsRuntime: "需绑定运行时",
    archived: "已归档",
    noChatsYet: "还没有聊天。",
    deleteChatTitle: "删除这次聊天？",
    deleteChatDescriptionTemplate: '“{title}”及其消息会被永久删除，无法撤销。',
    cancel: "取消",
    delete: "删除",
    messageNotSent: "消息未发送",
    permissionAlertTitle: "没有运行该智能体的权限",
    permissionAlertDescription:
      "你已没有运行该智能体的权限，消息没有发送。请向智能体所有者申请访问权限。",
    runtimeRequiredTitle: "需要运行时",
    runtimeRequiredAlertDescription:
      "请先在 Web 或桌面端为该智能体绑定运行时，再发送消息。",
    noAgentBannerTitle: "暂无可用智能体",
    noAgentBannerDescription: "请在“更多 → 智能体”中添加或启用智能体后开始聊天。",
    noAgentBannerA11y: "暂无可用智能体，打开智能体设置",
    runtimeFallbackName: "该智能体",
    runtimeRequiredBannerTemplate:
      "{name} 需要运行时才能运行。请在 Web 或桌面端绑定运行时。",
    offlineFallbackName: "该智能体",
    offlineUnstableTemplate: "{name} 可能刚刚断开连接，消息会排队等待。",
    offlineTemplate: "{name} 当前离线，消息会等运行时恢复后发送。",
    inputWorking: "智能体工作中…",
    inputMessage: "输入消息…",
    inputUnavailable: "聊天不可用",
    stopAgent: "停止智能体",
    noAgentSelected: "未选择智能体",
    accessRevoked: "你已没有运行该智能体的权限",
    noAgentsWorkspace: "此工作区没有智能体",
    archivedChat: "此聊天已归档",
    agentNeedsRuntime: "智能体需要运行时",
    emptyTitleNamed: "你好，我是 {name}",
    emptyTitleDefault: "和你的智能体对话",
    emptyFirstTimeHint: "选择一个示例开始，然后在发送前编辑内容。",
    fallbackStarters: ZH_STARTERS,
    suggestedFollowUps: "后续提问建议",
    showErrorDetails: "查看错误详情",
    showDetails: "查看详情",
    noResponse: "本轮已结束，智能体没有返回文字回复。",
    repliedInTemplate: "在 {elapsed} 内回复",
    finishedInTemplate: "在 {elapsed} 内结束",
    failedAfterTemplate: "在 {elapsed} 后失败",
    processStepOne: "1 步",
    processStepsOther: "{count} 步",
    queueTitleTemplate: "队列中有 {count} 条消息",
    queueFallback: "队列消息",
    toolFallback: "工具",
    toolResultNamedTemplate: "{tool} 结果：",
    toolResultUnnamed: "结果：",
    truncated: "（已截断）",
    toolState: {
      "approval-requested": "等待批准",
      "approval-responded": "已处理批准",
      "input-available": "运行中",
      "input-streaming": "等待输入",
      "output-available": "已完成",
      "output-denied": "已拒绝",
      "output-error": "出错",
    },
    status: {
      retrying: "重试中",
      offline: "离线",
      reconnecting: "重新连接中",
      queued: "排队中",
      startingUp: "启动中",
      thinking: "思考中",
      typing: "输入中",
      working: "工作中",
      runningCommand: "正在运行命令",
      readingFiles: "正在读取文件",
      searchingCode: "正在搜索代码",
      makingEdits: "正在编辑",
      searchingWeb: "正在搜索网页",
    },
    longPress: { copy: "复制", selectText: "选择文本", cancel: "取消" },
    failure: { fallback: "失败", labels: ZH_FAILURE_LABELS },
    sendFailure: {
      invocationNotAllowed: "你已没有运行该智能体的权限，消息没有发送。",
      runtimeRequired: "请先为该智能体绑定运行时，再发送消息。",
      fallback: "消息发送失败，请重试。",
    },
  },
} satisfies Record<ChatLocale, ChatCopyData>;

function interpolate(template: string, values: Record<string, string | number>): string {
  return Object.entries(values).reduce(
    (result, [key, value]) => result.replace(`{${key}}`, String(value)),
    template,
  );
}

function buildChatCopy(data: ChatCopyData): ChatCopy {
  return {
    ...data,
    deleteChatDescription: (title) =>
      interpolate(data.deleteChatDescriptionTemplate, { title }),
    runtimeRequiredBanner: (name) =>
      interpolate(data.runtimeRequiredBannerTemplate, { name }),
    offlineUnstable: (name) => interpolate(data.offlineUnstableTemplate, { name }),
    offline: (name) => interpolate(data.offlineTemplate, { name }),
    emptyTitle: (agentName) =>
      agentName
        ? interpolate(data.emptyTitleNamed, { name: agentName })
        : data.emptyTitleDefault,
    openChatWith: (name) => interpolate(data.openChatWithTemplate, { name }),
    repliedIn: (elapsed) => interpolate(data.repliedInTemplate, { elapsed }),
    finishedIn: (elapsed) => interpolate(data.finishedInTemplate, { elapsed }),
    failedAfter: (elapsed) => interpolate(data.failedAfterTemplate, { elapsed }),
    processSteps: (count) =>
      count === 1
        ? data.processStepOne
        : interpolate(data.processStepsOther, { count }),
    queueTitle: (count) => interpolate(data.queueTitleTemplate, { count }),
    toolResultNamed: (tool) =>
      interpolate(data.toolResultNamedTemplate, { tool }),
  };
}

export function normalizeChatLocale(language: string | null | undefined): ChatLocale {
  const normalized = language?.trim().toLowerCase().replaceAll("_", "-");
  if (normalized?.startsWith("zh")) return "zh-Hans";
  return "en";
}

export function createChatCopy(
  language: string | null | undefined,
): ChatCopy {
  return buildChatCopy(COPY_DATA[normalizeChatLocale(language)]);
}
