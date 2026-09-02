/**
 * A Weixin iLink installation bound to one Patchbay agent.
 *
 * The fields mirror WeixinInstallationResponse in the Go HTTP handler. The
 * status is intentionally open-ended so a newer server can add a terminal
 * state without crashing an older client.
 */
export interface WeixinInstallation {
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
}

export interface ListWeixinInstallationsResponse {
  installations: WeixinInstallation[];
  /** Whether the deployment has the at-rest key needed for Weixin installs. */
  configured: boolean;
  /** Whether the QR install flow is available in this deployment. */
  install_supported?: boolean;
}

export interface BeginWeixinInstallResponse {
  session_id: string;
  qr_code_url: string;
  expires_in_seconds: number;
  poll_interval_seconds: number;
}

export type WeixinInstallStatus =
  | "pending"
  | "scanned"
  | "need_verify_code"
  | "already_connected"
  | "expired"
  | "success"
  | string;

export interface WeixinInstallStatusResponse {
  status: WeixinInstallStatus;
  installation_id?: string;
}

export interface RedeemWeixinBindingTokenResponse {
  workspace_id: string;
  installation_id: string;
  weixin_user_id: string;
}
