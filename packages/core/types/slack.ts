import type {
  MessagingInstallationRuntime,
  MessagingInstallationSetup,
} from "./messaging";

/** A Slack bot installation, optionally bound to a Patchbay agent (PB-3666).
 *
 * Wire shape mirrors `SlackInstallationResponse` in
 * the Rust Slack handler. New fields the backend adds in the
 * future MUST default to optional so older desktop builds keep parsing the
 * response — see AGENTS.md → API Compatibility. */
export interface SlackInstallation {
  id: string;
  workspace_id: string;
  /** Null for a workspace Hub; the channel selects an Agent with /agents. */
  agent_id: string | null;
  /** The Slack workspace (team) id this bot is installed in. */
  team_id: string;
  /** The installed bot's Slack user id. */
  bot_user_id: string;
  installer_user_id: string;
  status: "active" | "revoked" | string;
  installed_at: string;
  created_at: string;
  updated_at: string;
  runtime?: MessagingInstallationRuntime;
  setup?: MessagingInstallationSetup;
}

export interface ListSlackInstallationsResponse {
  installations: SlackInstallation[];
  /** Whether the deployment has the at-rest secret key configured. When false
   * the connect entry points are hidden and the panel renders an "ask the
   * operator to enable Slack" state. */
  configured: boolean;
  /** Whether the deployment's selected setup path is ready. Managed mode
   * requires the hosted OAuth client, signing secret, callback, and at-rest
   * key; server-configured mode remains read-only in the App. */
  install_supported?: boolean;
  setup_mode?: "managed_oauth" | "server_configured" | string;
}

export interface BeginSlackOAuthRequest {
  /** Public-app path or same-origin HTTPS URL used after Slack returns. */
  redirect_url: string;
}

export interface BeginSlackOAuthResponse {
  authorization_url: string;
}

/** Request body for a bring-your-own-app (BYO) install: the two tokens the
 * admin pastes from the Slack app they created. The backend validates that both
 * belong to the same Slack app (and that the app token is live) before
 * persisting, then returns the created SlackInstallation. */
export interface RegisterSlackBYORequest {
  bot_token: string;
  app_token: string;
}

/** Post-redemption echo: the Slack user id the token carried is now bound to
 * the logged-in Patchbay user in this workspace/installation. */
export interface RedeemSlackBindingTokenResponse {
  workspace_id: string;
  installation_id: string;
  slack_user_id: string;
}
