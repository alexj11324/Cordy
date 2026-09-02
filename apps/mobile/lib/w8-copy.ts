export type W8Locale = "en" | "ja" | "ko" | "zh-Hans";

type ChannelCopy = {
  title: string;
  newChannel: string;
  emptyTitle: string;
  emptyDescription: string;
  retry: string;
  selectPrompt: string;
  createTitle: string;
  createDescription: string;
  name: string;
  namePlaceholder: string;
  slug: string;
  slugPlaceholder: string;
  description: string;
  descriptionPlaceholder: string;
  cancel: string;
  create: string;
  creating: string;
  loadEarlier: string;
  loadingEarlier: string;
  messagesEmpty: string;
  messagePlaceholder: string;
  send: string;
  required: string;
  createFailed: string;
  messageFailed: string;
  loadFailed: string;
};

type WecomCopy = {
  integrations: string;
  title: string;
  settingsSubtitle: string;
  notEnabledTitle: string;
  notEnabledDescription: string;
  unsupportedTitle: string;
  unsupportedDescription: string;
  previewTitle: string;
  previewDescription: string;
  connectedBots: string;
  loading: string;
  emptyTitle: string;
  emptyDescription: string;
  connect: string;
  disconnect: string;
  disconnectTitle: string;
  disconnectDescription: string;
  cancel: string;
  connecting: string;
  connected: string;
  revoked: string;
  revokeFailed: string;
  selectAgent: string;
  noAgents: string;
  botId: string;
  secret: string;
  botName: string;
  botNamePlaceholder: string;
  connectHelp: string;
  adminOnly: string;
  installSuccess: string;
  required: string;
  failed: string;
};

type BindCopy = {
  title: string;
  missingToken: string;
  signInRequired: string;
  signIn: string;
  redeeming: string;
  successTitle: string;
  successDescription: string;
  expired: string;
  conflict: string;
  notMember: string;
  failed: string;
  openAgain: string;
};

export type W8Copy = {
  channel: ChannelCopy;
  wecom: WecomCopy;
  bind: BindCopy;
};

const EN: W8Copy = {
  channel: {
    title: "Channels",
    newChannel: "New channel",
    emptyTitle: "No channels yet",
    emptyDescription: "Create a workspace channel for focused team updates.",
    retry: "Retry",
    selectPrompt: "Choose a channel to see its messages.",
    createTitle: "New channel",
    createDescription: "Create a shared conversation for this workspace.",
    name: "Name",
    namePlaceholder: "Team updates",
    slug: "Slug",
    slugPlaceholder: "team-updates",
    description: "Description",
    descriptionPlaceholder: "What is this channel for?",
    cancel: "Cancel",
    create: "Create",
    creating: "Creating…",
    loadEarlier: "Load earlier messages",
    loadingEarlier: "Loading earlier…",
    messagesEmpty: "No messages yet",
    messagePlaceholder: "Write a message…",
    send: "Send message",
    required: "This field is required.",
    createFailed: "Couldn’t create the channel.",
    messageFailed: "Couldn’t send the message.",
    loadFailed: "Couldn’t load channels.",
  },
  wecom: {
    integrations: "Integrations",
    title: "WeCom",
    settingsSubtitle: "Connect a WeCom smart bot to an agent",
    notEnabledTitle: "WeCom is not enabled",
    notEnabledDescription: "Ask your Patchbay operator to enable WeCom for this deployment.",
    unsupportedTitle: "WeCom install is unavailable",
    unsupportedDescription: "This deployment can list existing bots but cannot connect a new one.",
    previewTitle: "WeCom smart bots",
    previewDescription: "Connect a bot from the WeCom admin console to route messages to a Patchbay agent.",
    connectedBots: "Connected bots",
    loading: "Loading WeCom settings…",
    emptyTitle: "No WeCom bots connected",
    emptyDescription: "An owner or admin can connect a bot below.",
    connect: "Connect bot",
    disconnect: "Disconnect",
    disconnectTitle: "Disconnect WeCom bot?",
    disconnectDescription: "New messages from this bot will stop reaching Patchbay.",
    cancel: "Cancel",
    connecting: "Connecting…",
    connected: "Connected",
    revoked: "Revoked",
    revokeFailed: "Couldn’t disconnect this bot.",
    selectAgent: "Select an agent",
    noAgents: "No agents are available in this workspace.",
    botId: "Bot ID",
    secret: "Long-connection secret",
    botName: "Bot name (optional)",
    botNamePlaceholder: "Patchbay Bot",
    connectHelp: "Copy the Bot ID and long-connection secret from the WeCom admin console. The secret is encrypted before it is stored.",
    adminOnly: "Only workspace owners and admins can connect or disconnect bots.",
    installSuccess: "WeCom bot connected.",
    required: "Choose an agent, then enter the Bot ID and secret.",
    failed: "Couldn’t connect this WeCom bot.",
  },
  bind: {
    title: "Link WeCom account",
    missingToken: "This WeCom link is missing its binding token.",
    signInRequired: "Sign in to Patchbay, then open the WeCom link again to finish linking your account.",
    signIn: "Sign in",
    redeeming: "Linking your WeCom account…",
    successTitle: "WeCom account linked",
    successDescription: "Your WeCom account is now linked to your Patchbay account.",
    expired: "This link is invalid or has expired. Ask WeCom for a new link.",
    conflict: "This WeCom account is already linked to another Patchbay user.",
    notMember: "Your Patchbay account is not a member of the target workspace.",
    failed: "We couldn’t link this WeCom account. Try again with a fresh link.",
    openAgain: "Open the link again after signing in.",
  },
};

const JA: W8Copy = {
  channel: {
    title: "チャンネル",
    newChannel: "新しいチャンネル",
    emptyTitle: "チャンネルはまだありません",
    emptyDescription: "チームの更新を共有するワークスペースチャンネルを作成します。",
    retry: "再試行",
    selectPrompt: "チャンネルを選ぶとメッセージが表示されます。",
    createTitle: "新しいチャンネル",
    createDescription: "このワークスペースの共有会話を作成します。",
    name: "名前",
    namePlaceholder: "チームの更新",
    slug: "スラッグ",
    slugPlaceholder: "team-updates",
    description: "説明",
    descriptionPlaceholder: "このチャンネルの用途",
    cancel: "キャンセル",
    create: "作成",
    creating: "作成中…",
    loadEarlier: "以前のメッセージを読み込む",
    loadingEarlier: "読み込み中…",
    messagesEmpty: "メッセージはまだありません",
    messagePlaceholder: "メッセージを書く…",
    send: "メッセージを送信",
    required: "この項目は必須です。",
    createFailed: "チャンネルを作成できませんでした。",
    messageFailed: "メッセージを送信できませんでした。",
    loadFailed: "チャンネルを読み込めませんでした。",
  },
  wecom: {
    integrations: "連携",
    title: "WeCom",
    settingsSubtitle: "WeComスマートボットをエージェントに接続",
    notEnabledTitle: "WeComは有効になっていません",
    notEnabledDescription: "この環境でWeComを有効にするようPatchbayの管理者に依頼してください。",
    unsupportedTitle: "WeComのインストールは利用できません",
    unsupportedDescription: "既存のボットは表示できますが、新しいボットは接続できません。",
    previewTitle: "WeComスマートボット",
    previewDescription: "WeCom管理コンソールのボットを接続し、メッセージをPatchbayエージェントに送ります。",
    connectedBots: "接続済みボット",
    loading: "WeCom設定を読み込み中…",
    emptyTitle: "接続済みのWeComボットはありません",
    emptyDescription: "オーナーまたは管理者が下からボットを接続できます。",
    connect: "ボットを接続",
    disconnect: "接続を解除",
    disconnectTitle: "WeComボットの接続を解除しますか？",
    disconnectDescription: "このボットからの新しいメッセージはPatchbayに届かなくなります。",
    cancel: "キャンセル",
    connecting: "接続中…",
    connected: "接続済み",
    revoked: "解除済み",
    revokeFailed: "ボットの接続を解除できませんでした。",
    selectAgent: "エージェントを選択",
    noAgents: "このワークスペースには利用できるエージェントがありません。",
    botId: "ボットID",
    secret: "長時間接続シークレット",
    botName: "ボット名（任意）",
    botNamePlaceholder: "Patchbay Bot",
    connectHelp: "WeCom管理コンソールからボットIDと長時間接続シークレットをコピーしてください。シークレットは暗号化して保存されます。",
    adminOnly: "ボットの接続と解除はワークスペースのオーナーと管理者のみ可能です。",
    installSuccess: "WeComボットを接続しました。",
    required: "エージェントを選び、ボットIDとシークレットを入力してください。",
    failed: "WeComボットを接続できませんでした。",
  },
  bind: {
    title: "WeComアカウントをリンク",
    missingToken: "このWeComリンクにはバインドトークンがありません。",
    signInRequired: "Patchbayにサインインしてから、WeComリンクをもう一度開いてリンクを完了してください。",
    signIn: "サインイン",
    redeeming: "WeComアカウントをリンク中…",
    successTitle: "WeComアカウントをリンクしました",
    successDescription: "WeComアカウントがPatchbayアカウントにリンクされました。",
    expired: "このリンクは無効か期限切れです。WeComから新しいリンクを取得してください。",
    conflict: "このWeComアカウントは別のPatchbayユーザーにリンクされています。",
    notMember: "Patchbayアカウントは対象ワークスペースのメンバーではありません。",
    failed: "WeComアカウントをリンクできませんでした。新しいリンクで再試行してください。",
    openAgain: "サインイン後にリンクをもう一度開いてください。",
  },
};

const KO: W8Copy = {
  channel: {
    title: "채널",
    newChannel: "새 채널",
    emptyTitle: "아직 채널이 없습니다",
    emptyDescription: "팀 업데이트를 공유할 워크스페이스 채널을 만드세요.",
    retry: "다시 시도",
    selectPrompt: "채널을 선택하면 메시지가 표시됩니다.",
    createTitle: "새 채널",
    createDescription: "이 워크스페이스의 공유 대화를 만듭니다.",
    name: "이름",
    namePlaceholder: "팀 업데이트",
    slug: "슬러그",
    slugPlaceholder: "team-updates",
    description: "설명",
    descriptionPlaceholder: "이 채널의 용도",
    cancel: "취소",
    create: "만들기",
    creating: "만드는 중…",
    loadEarlier: "이전 메시지 불러오기",
    loadingEarlier: "불러오는 중…",
    messagesEmpty: "아직 메시지가 없습니다",
    messagePlaceholder: "메시지 작성…",
    send: "메시지 보내기",
    required: "필수 입력 항목입니다.",
    createFailed: "채널을 만들 수 없습니다.",
    messageFailed: "메시지를 보낼 수 없습니다.",
    loadFailed: "채널을 불러올 수 없습니다.",
  },
  wecom: {
    integrations: "연동",
    title: "WeCom",
    settingsSubtitle: "WeCom 스마트 봇을 에이전트에 연결",
    notEnabledTitle: "WeCom이 활성화되지 않았습니다",
    notEnabledDescription: "이 배포에서 WeCom을 활성화하도록 Patchbay 운영자에게 요청하세요.",
    unsupportedTitle: "WeCom 설치를 사용할 수 없습니다",
    unsupportedDescription: "기존 봇은 볼 수 있지만 새 봇을 연결할 수 없습니다.",
    previewTitle: "WeCom 스마트 봇",
    previewDescription: "WeCom 관리자 콘솔의 봇을 연결해 메시지를 Patchbay 에이전트로 전달하세요.",
    connectedBots: "연결된 봇",
    loading: "WeCom 설정 불러오는 중…",
    emptyTitle: "연결된 WeCom 봇이 없습니다",
    emptyDescription: "소유자 또는 관리자가 아래에서 봇을 연결할 수 있습니다.",
    connect: "봇 연결",
    disconnect: "연결 해제",
    disconnectTitle: "WeCom 봇 연결을 해제할까요?",
    disconnectDescription: "이 봇의 새 메시지는 Patchbay에 도착하지 않습니다.",
    cancel: "취소",
    connecting: "연결 중…",
    connected: "연결됨",
    revoked: "해제됨",
    revokeFailed: "봇 연결을 해제할 수 없습니다.",
    selectAgent: "에이전트 선택",
    noAgents: "이 워크스페이스에 사용할 수 있는 에이전트가 없습니다.",
    botId: "봇 ID",
    secret: "장기 연결 시크릿",
    botName: "봇 이름(선택 사항)",
    botNamePlaceholder: "Patchbay Bot",
    connectHelp: "WeCom 관리자 콘솔에서 봇 ID와 장기 연결 시크릿을 복사하세요. 시크릿은 암호화되어 저장됩니다.",
    adminOnly: "워크스페이스 소유자와 관리자만 봇을 연결하거나 해제할 수 있습니다.",
    installSuccess: "WeCom 봇이 연결되었습니다.",
    required: "에이전트를 선택하고 봇 ID와 시크릿을 입력하세요.",
    failed: "WeCom 봇을 연결할 수 없습니다.",
  },
  bind: {
    title: "WeCom 계정 연결",
    missingToken: "이 WeCom 링크에 연결 토큰이 없습니다.",
    signInRequired: "Patchbay에 로그인한 뒤 WeCom 링크를 다시 열어 계정 연결을 완료하세요.",
    signIn: "로그인",
    redeeming: "WeCom 계정 연결 중…",
    successTitle: "WeCom 계정이 연결되었습니다",
    successDescription: "WeCom 계정이 Patchbay 계정에 연결되었습니다.",
    expired: "링크가 유효하지 않거나 만료되었습니다. WeCom에서 새 링크를 요청하세요.",
    conflict: "이 WeCom 계정은 다른 Patchbay 사용자에게 이미 연결되어 있습니다.",
    notMember: "Patchbay 계정이 대상 워크스페이스의 멤버가 아닙니다.",
    failed: "WeCom 계정을 연결할 수 없습니다. 새 링크로 다시 시도하세요.",
    openAgain: "로그인 후 링크를 다시 여세요.",
  },
};

const ZH_HANS: W8Copy = {
  channel: {
    title: "频道",
    newChannel: "新建频道",
    emptyTitle: "还没有频道",
    emptyDescription: "创建一个工作区频道，集中分享团队动态。",
    retry: "重试",
    selectPrompt: "选择一个频道查看消息。",
    createTitle: "新建频道",
    createDescription: "为这个工作区创建共享对话。",
    name: "名称",
    namePlaceholder: "团队动态",
    slug: "标识",
    slugPlaceholder: "team-updates",
    description: "描述",
    descriptionPlaceholder: "这个频道用于什么？",
    cancel: "取消",
    create: "创建",
    creating: "创建中…",
    loadEarlier: "加载更早消息",
    loadingEarlier: "加载中…",
    messagesEmpty: "还没有消息",
    messagePlaceholder: "写消息…",
    send: "发送消息",
    required: "此项为必填项。",
    createFailed: "无法创建频道。",
    messageFailed: "无法发送消息。",
    loadFailed: "无法加载频道。",
  },
  wecom: {
    integrations: "集成",
    title: "企业微信",
    settingsSubtitle: "将企业微信智能机器人连接到 Agent",
    notEnabledTitle: "企业微信未启用",
    notEnabledDescription: "请联系 Patchbay 运营方为此部署启用企业微信。",
    unsupportedTitle: "企业微信安装不可用",
    unsupportedDescription: "可以查看已有机器人，但此部署无法连接新机器人。",
    previewTitle: "企业微信智能机器人",
    previewDescription: "从企业微信管理后台连接机器人，将消息路由到 Patchbay Agent。",
    connectedBots: "已连接机器人",
    loading: "正在加载企业微信设置…",
    emptyTitle: "还没有连接企业微信机器人",
    emptyDescription: "工作区所有者或管理员可以在下方连接机器人。",
    connect: "连接机器人",
    disconnect: "断开连接",
    disconnectTitle: "断开企业微信机器人？",
    disconnectDescription: "此机器人发来的新消息将不再进入 Patchbay。",
    cancel: "取消",
    connecting: "连接中…",
    connected: "已连接",
    revoked: "已断开",
    revokeFailed: "无法断开此机器人。",
    selectAgent: "选择 Agent",
    noAgents: "此工作区没有可用 Agent。",
    botId: "机器人 ID",
    secret: "长连接 Secret",
    botName: "机器人名称（可选）",
    botNamePlaceholder: "Patchbay Bot",
    connectHelp: "从企业微信管理后台复制机器人 ID 和长连接 Secret。Secret 加密后才会存储。",
    adminOnly: "只有工作区所有者和管理员可以连接或断开机器人。",
    installSuccess: "企业微信机器人已连接。",
    required: "请选择 Agent，然后输入机器人 ID 和 Secret。",
    failed: "无法连接此企业微信机器人。",
  },
  bind: {
    title: "绑定企业微信账号",
    missingToken: "此企业微信链接缺少绑定 Token。",
    signInRequired: "请先登录 Patchbay，再重新打开企业微信链接完成绑定。",
    signIn: "登录",
    redeeming: "正在绑定企业微信账号…",
    successTitle: "企业微信账号已绑定",
    successDescription: "你的企业微信账号现在已关联到 Patchbay 账号。",
    expired: "此链接无效或已过期，请从企业微信获取新链接。",
    conflict: "此企业微信账号已绑定到其他 Patchbay 用户。",
    notMember: "你的 Patchbay 账号不是目标工作区成员。",
    failed: "无法绑定此企业微信账号，请使用新链接重试。",
    openAgain: "登录后请重新打开链接。",
  },
};

const COPY: Record<W8Locale, W8Copy> = {
  en: EN,
  ja: JA,
  ko: KO,
  "zh-Hans": ZH_HANS,
};

export function normalizeW8Locale(language: string | null | undefined): W8Locale {
  if (language?.toLowerCase().startsWith("zh")) return "zh-Hans";
  if (language === "ja" || language === "ko") return language;
  return "en";
}

export function getW8Copy(language: string | null | undefined): W8Copy {
  return COPY[normalizeW8Locale(language)];
}

export const W8_COPY_LOCALES = Object.keys(COPY) as W8Locale[];
