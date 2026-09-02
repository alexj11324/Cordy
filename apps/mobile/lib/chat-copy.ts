import type { AgentConversationStarter } from "@patchbay/core/types";
import type { DispatchReasonCopy } from "@/lib/dispatch-reason";

export type ChatLocale = "en" | "zh-Hans" | "ja" | "ko";
type ConversationStarter = Pick<AgentConversationStarter, "label" | "prompt">;

export type ChatCopy = {
  chat: string;
  newChat: string;
  sessionsAndAgentPicker: string;
  sessionActions: string;
  chooseAgent: string;
  noAgentsAvailable: string;
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
  toolFallback: string;
  toolResultNamed: (tool: string) => string;
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
  | "repliedIn"
  | "finishedIn"
  | "failedAfter"
  | "processSteps"
  | "toolResultNamed"
> & {
  deleteChatDescriptionTemplate: string;
  runtimeRequiredBannerTemplate: string;
  offlineUnstableTemplate: string;
  offlineTemplate: string;
  emptyTitleNamed: string;
  emptyTitleDefault: string;
  repliedInTemplate: string;
  finishedInTemplate: string;
  failedAfterTemplate: string;
  processStepOne: string;
  processStepsOther: string;
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

const JA_FAILURE_LABELS = {
  queued_expired: "キューで期限切れ",
  runtime_offline: "ランタイムがオフライン",
  runtime_recovery: "ランタイムが再起動",
  timeout: "タスクがタイムアウト",
  iteration_limit: "反復上限に到達",
  agent_blocked: "人の入力を待機中",
  api_invalid_request: "モデル API に拒否されました",
  skill_bundle_unavailable: "エージェントのスキルを取得できません",
  runtime_cli_timeout: "ローカルランタイム CLI がタイムアウト",
  "agent_error.provider_auth_or_access": "プロバイダー認証に失敗",
  "agent_error.provider_quota_limit": "プロバイダーの利用枠を使い切りました",
  "agent_error.provider_capacity_or_rate_limit": "プロバイダーにより制限中",
  "agent_error.provider_server_error": "プロバイダーサーバーエラー",
  "agent_error.provider_network": "プロバイダーへの接続エラー",
  "agent_error.process_failure": "エージェントプロセスがクラッシュ",
  "agent_error.empty_or_unparseable_output": "利用できる出力がありません",
  "agent_error.agent_timeout": "エージェントがタイムアウト",
  "agent_error.context_overflow": "コンテキスト上限を超過",
  "agent_error.missing_config": "API キーまたは設定がありません",
  "agent_error.model_not_found_or_unavailable": "モデルを利用できません",
  "agent_error.runtime_version_unsupported": "ランナー CLI は未対応のバージョン",
  "agent_error.runtime_missing_executable": "ランナー CLI が未インストール",
  "agent_error.unknown": "エージェント実行エラー",
  agent_error: "エージェント実行エラー",
  codex_semantic_inactivity: "Codex が応答を停止",
  manual: "ユーザーがキャンセル",
} satisfies Readonly<Record<string, string>>;

const KO_FAILURE_LABELS = {
  queued_expired: "대기열에서 만료됨",
  runtime_offline: "런타임 오프라인",
  runtime_recovery: "런타임 재시작됨",
  timeout: "태스크 시간 초과",
  iteration_limit: "반복 한도에 도달함",
  agent_blocked: "사용자 입력 대기 중",
  api_invalid_request: "모델 API에서 거부됨",
  skill_bundle_unavailable: "에이전트 스킬을 다운로드하지 못함",
  runtime_cli_timeout: "로컬 런타임 CLI 시간 초과",
  "agent_error.provider_auth_or_access": "모델 서비스 인증 실패",
  "agent_error.provider_quota_limit": "모델 서비스 할당량 소진",
  "agent_error.provider_capacity_or_rate_limit": "모델 서비스 요청 제한",
  "agent_error.provider_server_error": "모델 서비스 서버 오류",
  "agent_error.provider_network": "모델 서비스 연결 오류",
  "agent_error.process_failure": "에이전트 프로세스 충돌",
  "agent_error.empty_or_unparseable_output": "사용할 수 있는 출력이 없음",
  "agent_error.agent_timeout": "에이전트 시간 초과",
  "agent_error.context_overflow": "컨텍스트 창 초과",
  "agent_error.missing_config": "API 키 또는 설정 없음",
  "agent_error.model_not_found_or_unavailable": "모델을 사용할 수 없음",
  "agent_error.runtime_version_unsupported": "러너 CLI 버전 미지원",
  "agent_error.runtime_missing_executable": "러너 CLI가 설치되지 않음",
  "agent_error.unknown": "에이전트 실행 오류",
  agent_error: "에이전트 실행 오류",
  codex_semantic_inactivity: "Codex 응답 없음",
  manual: "사용자가 취소함",
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

const JA_STARTERS: readonly ConversationStarter[] = [
  {
    label: "何を手伝えますか？",
    prompt: "あなたが得意な支援を簡潔に教えてください。",
  },
  {
    label: "最初のタスクを提案",
    prompt: "あなたに任せられる実用的なタスクを3つ提案してください。",
  },
  {
    label: "アクションを提案",
    prompt:
      "私のワークスペースについて知っていることを踏まえ、最初に役立つアクションを提案してください。",
  },
];

const KO_STARTERS: readonly ConversationStarter[] = [
  {
    label: "어떤 일을 도와줄 수 있나요?",
    prompt: "가장 잘 도울 수 있는 일을 간단히 알려 주세요.",
  },
  {
    label: "첫 태스크 추천",
    prompt: "당신에게 맡길 수 있는 유용한 태스크 세 가지를 추천해 주세요.",
  },
  {
    label: "작업 추천",
    prompt:
      "내 워크스페이스에 대해 알고 있는 내용을 바탕으로 유용한 첫 작업을 추천해 주세요.",
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
    toolFallback: "tool",
    toolResultNamedTemplate: "{tool} result: ",
    toolResultUnnamed: "result: ",
    truncated: "(truncated)",
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
    toolFallback: "工具",
    toolResultNamedTemplate: "{tool} 结果：",
    toolResultUnnamed: "结果：",
    truncated: "（已截断）",
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
  ja: {
    chat: "チャット",
    newChat: "新規チャット",
    sessionsAndAgentPicker: "セッションとエージェントの選択",
    sessionActions: "セッション操作",
    chooseAgent: "エージェントを選択",
    noAgentsAvailable: "利用できるエージェントはありません。",
    needsRuntime: "ランタイム未設定",
    archived: "アーカイブ済み",
    noChatsYet: "チャットはまだありません。",
    deleteChatTitle: "このチャットを削除しますか？",
    deleteChatDescriptionTemplate:
      "「{title}」とメッセージが完全に削除されます。この操作は取り消せません。",
    cancel: "キャンセル",
    delete: "削除",
    messageNotSent: "メッセージを送信できませんでした",
    permissionAlertTitle: "このエージェントを実行する権限がありません",
    permissionAlertDescription:
      "このエージェントを実行する権限がないため、メッセージは送信されませんでした。所有者にアクセスを依頼してください。",
    runtimeRequiredTitle: "ランタイムが必要です",
    runtimeRequiredAlertDescription:
      "メッセージを送る前に、Web またはデスクトップでこのエージェントにランタイムを設定してください。",
    noAgentBannerTitle: "利用できるエージェントはありません",
    noAgentBannerDescription:
      "チャットを始めるには「その他 → エージェント」で追加または有効化してください。",
    noAgentBannerA11y: "利用できるエージェントはありません。エージェント設定を開く",
    runtimeFallbackName: "このエージェント",
    runtimeRequiredBannerTemplate:
      "{name} を実行するにはランタイムが必要です。Web またはデスクトップで設定してください。",
    offlineFallbackName: "このエージェント",
    offlineUnstableTemplate: "{name} は接続が切れた可能性があります。メッセージはキューに入ります。",
    offlineTemplate: "{name} はオフラインです。ランタイムが戻るまでメッセージは待機します。",
    inputWorking: "エージェントが作業中…",
    inputMessage: "メッセージ…",
    inputUnavailable: "チャットを利用できません",
    stopAgent: "エージェントを停止",
    noAgentSelected: "エージェント未選択",
    accessRevoked: "このエージェントを実行する権限がありません",
    noAgentsWorkspace: "このワークスペースにエージェントがありません",
    archivedChat: "このチャットはアーカイブ済みです",
    agentNeedsRuntime: "エージェントにランタイムが必要です",
    emptyTitleNamed: "こんにちは、{name} です",
    emptyTitleDefault: "エージェントとチャット",
    emptyFirstTimeHint: "例を選んで始め、送信前に編集してください。",
    fallbackStarters: JA_STARTERS,
    suggestedFollowUps: "おすすめのフォローアップ",
    showErrorDetails: "エラーの詳細を表示",
    showDetails: "詳細を表示",
    noResponse: "このターンはテキスト返信なしで終了しました。",
    repliedInTemplate: "{elapsed} で返信",
    finishedInTemplate: "{elapsed} で完了",
    failedAfterTemplate: "{elapsed} 後に失敗",
    processStepOne: "1 ステップ",
    processStepsOther: "{count} ステップ",
    toolFallback: "ツール",
    toolResultNamedTemplate: "{tool} の結果: ",
    toolResultUnnamed: "結果: ",
    truncated: "（省略）",
    status: {
      retrying: "再試行中",
      offline: "オフライン",
      reconnecting: "再接続中",
      queued: "待機中",
      startingUp: "起動中",
      thinking: "思考中",
      typing: "入力中",
      working: "作業中",
      runningCommand: "コマンドを実行中",
      readingFiles: "ファイルを読み込み中",
      searchingCode: "コードを検索中",
      makingEdits: "編集中",
      searchingWeb: "ウェブを検索中",
    },
    longPress: { copy: "コピー", selectText: "テキストを選択", cancel: "キャンセル" },
    failure: { fallback: "失敗しました", labels: JA_FAILURE_LABELS },
    sendFailure: {
      invocationNotAllowed: "このエージェントを実行する権限がないため、メッセージは送信されませんでした。",
      runtimeRequired: "メッセージを送る前に、このエージェントにランタイムを設定してください。",
      fallback: "メッセージを送信できませんでした。もう一度お試しください。",
    },
  },
  ko: {
    chat: "채팅",
    newChat: "새 채팅",
    sessionsAndAgentPicker: "세션 및 에이전트 선택",
    sessionActions: "세션 작업",
    chooseAgent: "에이전트 선택",
    noAgentsAvailable: "사용할 수 있는 에이전트가 없습니다.",
    needsRuntime: "런타임 필요",
    archived: "보관됨",
    noChatsYet: "아직 채팅이 없습니다.",
    deleteChatTitle: "이 채팅을 삭제할까요?",
    deleteChatDescriptionTemplate:
      '"{title}" 및 메시지가 영구 삭제됩니다. 이 작업은 되돌릴 수 없습니다.',
    cancel: "취소",
    delete: "삭제",
    messageNotSent: "메시지가 전송되지 않았습니다",
    permissionAlertTitle: "이 에이전트를 실행할 권한이 없습니다",
    permissionAlertDescription:
      "이 에이전트를 실행할 권한이 없어 메시지가 전송되지 않았습니다. 소유자에게 접근 권한을 요청하세요.",
    runtimeRequiredTitle: "런타임이 필요합니다",
    runtimeRequiredAlertDescription:
      "메시지를 보내기 전에 웹 또는 데스크톱에서 이 에이전트에 런타임을 연결하세요.",
    noAgentBannerTitle: "사용할 수 있는 에이전트가 없습니다",
    noAgentBannerDescription:
      "채팅을 시작하려면 더 보기 → 에이전트에서 에이전트를 추가하거나 활성화하세요.",
    noAgentBannerA11y: "사용할 수 있는 에이전트 없음, 에이전트 설정 열기",
    runtimeFallbackName: "이 에이전트",
    runtimeRequiredBannerTemplate:
      "{name}을(를) 실행하려면 런타임이 필요합니다. 웹 또는 데스크톱에서 연결하세요.",
    offlineFallbackName: "이 에이전트",
    offlineUnstableTemplate: "{name}의 연결이 끊겼을 수 있습니다. 메시지는 대기열에 들어갑니다.",
    offlineTemplate: "{name}이(가) 오프라인입니다. 런타임이 돌아올 때까지 메시지가 대기합니다.",
    inputWorking: "에이전트 작업 중…",
    inputMessage: "메시지…",
    inputUnavailable: "채팅을 사용할 수 없음",
    stopAgent: "에이전트 중지",
    noAgentSelected: "에이전트가 선택되지 않음",
    accessRevoked: "이 에이전트를 실행할 권한이 없습니다",
    noAgentsWorkspace: "이 워크스페이스에 에이전트가 없습니다",
    archivedChat: "이 채팅은 보관되었습니다",
    agentNeedsRuntime: "에이전트에 런타임이 필요함",
    emptyTitleNamed: "안녕하세요, 저는 {name}입니다",
    emptyTitleDefault: "에이전트와 채팅하기",
    emptyFirstTimeHint: "예시를 골라 시작한 다음 보내기 전에 편집하세요.",
    fallbackStarters: KO_STARTERS,
    suggestedFollowUps: "추천 후속 질문",
    showErrorDetails: "오류 세부 정보 보기",
    showDetails: "세부 정보 보기",
    noResponse: "에이전트가 텍스트 답변 없이 이 턴을 마쳤습니다.",
    repliedInTemplate: "{elapsed} 만에 답변",
    finishedInTemplate: "{elapsed} 만에 완료",
    failedAfterTemplate: "{elapsed} 후 실패",
    processStepOne: "1단계",
    processStepsOther: "{count}단계",
    toolFallback: "도구",
    toolResultNamedTemplate: "{tool} 결과: ",
    toolResultUnnamed: "결과: ",
    truncated: "(축약됨)",
    status: {
      retrying: "재시도 중",
      offline: "오프라인",
      reconnecting: "다시 연결 중",
      queued: "대기 중",
      startingUp: "시작 중",
      thinking: "생각 중",
      typing: "입력 중",
      working: "작업 중",
      runningCommand: "명령 실행 중",
      readingFiles: "파일 읽는 중",
      searchingCode: "코드 검색 중",
      makingEdits: "수정 중",
      searchingWeb: "웹 검색 중",
    },
    longPress: { copy: "복사", selectText: "텍스트 선택", cancel: "취소" },
    failure: { fallback: "실패", labels: KO_FAILURE_LABELS },
    sendFailure: {
      invocationNotAllowed: "이 에이전트를 실행할 권한이 없어 메시지가 전송되지 않았습니다.",
      runtimeRequired: "메시지를 보내기 전에 이 에이전트에 런타임을 연결하세요.",
      fallback: "메시지를 보내지 못했습니다. 다시 시도해 주세요.",
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
    repliedIn: (elapsed) => interpolate(data.repliedInTemplate, { elapsed }),
    finishedIn: (elapsed) => interpolate(data.finishedInTemplate, { elapsed }),
    failedAfter: (elapsed) => interpolate(data.failedAfterTemplate, { elapsed }),
    processSteps: (count) =>
      count === 1
        ? data.processStepOne
        : interpolate(data.processStepsOther, { count }),
    toolResultNamed: (tool) =>
      interpolate(data.toolResultNamedTemplate, { tool }),
  };
}

export function normalizeChatLocale(language: string | null | undefined): ChatLocale {
  const normalized = language?.trim().toLowerCase().replaceAll("_", "-");
  if (normalized?.startsWith("zh")) return "zh-Hans";
  if (normalized?.startsWith("ja")) return "ja";
  if (normalized?.startsWith("ko")) return "ko";
  return "en";
}

export function createChatCopy(
  language: string | null | undefined,
): ChatCopy {
  return buildChatCopy(COPY_DATA[normalizeChatLocale(language)]);
}
