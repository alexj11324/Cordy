export type WeixinInstallation = {
  id: string;
  workspace_id: string;
  agent_id: string;
  bot_id: string;
  ilink_user_id: string;
  installer_user_id: string;
  status: "active" | "revoked" | string;
  installed_at: string;
  created_at: string;
  updated_at: string;
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
