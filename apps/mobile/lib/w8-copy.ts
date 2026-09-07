export type W8Locale = "en" | "zh-Hans";

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
  installed: string;
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
    installed: "Installed",
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
    installed: "已安装",
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
  "zh-Hans": ZH_HANS,
};

export function normalizeW8Locale(language: string | null | undefined): W8Locale {
  if (language?.toLowerCase().startsWith("zh")) return "zh-Hans";
  return "en";
}

export function getW8Copy(language: string | null | undefined): W8Copy {
  return COPY[normalizeW8Locale(language)];
}

export const W8_COPY_LOCALES = Object.keys(COPY) as W8Locale[];
