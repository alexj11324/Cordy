export type WeixinInstallation = {
  id: string;
  workspace_id: string;
  /** Null for a workspace Hub; the channel selects an Agent with /agents. */
  agent_id: string | null;
  bot_id: string;
  ilink_user_id: string;
  installer_user_id: string;
  status: "active" | "revoked" | string;
  installed_at: string;
  created_at: string;
  updated_at: string;
  /** Provider authorization accepted during installation. */
  credential_status?: string;
  /** Current provider runtime health; older backends may omit it. */
  runtime_status?: string;
  /** Server-owned inbound -> outbound message verification state. */
  round_trip_status?: string;
  /** Action the UI should take when verification is incomplete. */
  required_action?: string;
};

export type ListWeixinInstallationsResponse = {
  installations: WeixinInstallation[];
  configured: boolean;
  install_supported?: boolean;
};

export type BeginWeixinInstallResponse = {
  session_id: string;
  qr_code_url: string;
  expires_in_seconds: number;
  poll_interval_seconds: number;
};

export type WeixinInstallStatusResponse = {
  status: string;
  installation_id?: string;
};

export type RedeemWeixinBindingTokenResponse = {
  workspace_id: string;
  installation_id: string;
  weixin_user_id: string;
};
