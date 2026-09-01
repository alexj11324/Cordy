/**
 * Server-authoritative hosted IM quota snapshot.
 *
 * `used`/`reserved` are nullable when the deployment does not expose a
 * trusted hosted quota (for example self-hosted mode). Clients must render
 * that as unavailable instead of assuming a Free or Pro entitlement.
 */
export interface MessagingQuotaUsage {
  mode: "managed" | "unlimited" | "disabled" | "unavailable" | string;
  used: number | null;
  reserved: number | null;
  limit: number | null;
  period_start: string | null;
  period_end: string | null;
  reset_at: string | null;
}

export type MessagingRuntimeState =
  | "starting"
  | "healthy"
  | "degraded"
  | "offline"
  | "error"
  | string;

/** Latest server-observed transport state for a channel installation. */
export interface MessagingInstallationRuntime {
  state: MessagingRuntimeState;
  observedAt: string | null;
  errorCode: string | null;
}

export type MessagingInstallationSetupMode =
  | "managed_oauth"
  | "managed_token"
  | "server_configured"
  | string;

/** How the installation is configured and whether this client may mutate it. */
export interface MessagingInstallationSetup {
  mode: MessagingInstallationSetupMode;
  writable: boolean;
  experimental: boolean;
}

/** A desired active row is not a live connection until the supervisor reports healthy. */
export function isMessagingInstallationHealthy(installation: {
  status: string;
  runtime?: MessagingInstallationRuntime;
  setup?: { experimental?: boolean };
}): boolean {
  return (
    installation.status === "active" &&
    installation.runtime?.state === "healthy" &&
    // A provider that has not passed the real install/bind/message/reply
    // exercise must never be presented as production-healthy, even if its
    // supervisor currently owns a lease.
    installation.setup?.experimental !== true
  );
}
