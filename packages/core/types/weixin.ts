import type {
  MessagingInstallationRuntime,
  MessagingInstallationSetup,
} from "./messaging";

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
  runtime?: MessagingInstallationRuntime;
  setup?: MessagingInstallationSetup;
};

export type ListWeixinInstallationsResponse = {
  installations: WeixinInstallation[];
  configured: boolean;
  install_supported?: boolean;
};

export type BeginWeixinInstallResponse = {
  session_id: string;
  /** Display payload returned by iLink (`qrcode_img_content`). */
  qr_code_content?: string;
  /** Kept for older clients; the server now puts the display payload here,
   * never the polling `qrcode` token. */
  qr_code_url: string;
  expires_in_seconds: number;
  poll_interval_seconds: number;
};

export type WeixinInstallStatusResponse = {
  status: string;
  installation_id?: string;
  /** Stable diagnostic code when the provider reports a non-success state. */
  errorCode?: string;
};

export type RedeemWeixinBindingTokenResponse = {
  workspace_id: string;
  installation_id: string;
  weixin_user_id: string;
};
