/** Server-observed connection status, separate from installation enablement. */
export type MessagingInstallationRuntime = {
  state: "starting" | "healthy" | "degraded" | "offline" | "error" | string;
  observedAt: string | null;
  errorCode: string | null;
  errorSummary?: string | null;
};

export type MessagingInstallationSetup = {
  mode: "managed_oauth" | "managed_token" | "server_configured" | string;
  writable: boolean;
  experimental: boolean;
};

export type MessagingConnectionSource = {
  status: string;
  runtime?: MessagingInstallationRuntime;
  setup?: MessagingInstallationSetup;
};

export type MessagingConnectionState =
  | "connected" | "connecting" | "disconnected" | "degraded" | "error"
  | "unavailable" | "paused" | "experimental";

export function messagingConnectionState(installation: MessagingConnectionSource): MessagingConnectionState {
  if (installation.status === "revoked") return "disconnected";
  if (installation.status !== "active") return "unavailable";
  const runtime = installation.runtime;
  if (runtime?.errorCode === "hosted_quota_paused") return "paused";
  switch (runtime?.state) {
    case "starting": return "connecting";
    case "offline": return "disconnected";
    case "degraded": return "degraded";
    case "error": return "error";
    case "healthy":
      if (!runtime.observedAt || Number.isNaN(Date.parse(runtime.observedAt))) return "unavailable";
      return installation.setup?.experimental === true ? "experimental" : "connected";
    default: return "unavailable";
  }
}

export function isMessagingInstallationConnected(installation: MessagingConnectionSource): boolean {
  return messagingConnectionState(installation) === "connected";
}
