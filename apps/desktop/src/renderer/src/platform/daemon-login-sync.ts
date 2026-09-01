import type {
  DaemonAutoStartResult,
  DaemonAutoStartFailureReason,
  DaemonStatus,
} from "../../../shared/daemon-types";

const DAEMON_READY_TIMEOUT_MS = 60_000;
const DAEMON_READY_POLL_MS = 250;

export type WaitForDaemonRunningOptions = {
  timeoutMs?: number;
  pollMs?: number;
  expectedProfile?: string;
};

export class DaemonLoginSyncError extends Error {
  readonly reason: DaemonAutoStartFailureReason;

  constructor(reason: DaemonAutoStartFailureReason, message: string) {
    super(message);
    this.name = "DaemonLoginSyncError";
    this.reason = reason;
  }
}

function asDaemonLoginSyncError(
  error: unknown,
  reason: DaemonAutoStartFailureReason = "start_failed",
): DaemonLoginSyncError {
  return error instanceof DaemonLoginSyncError
    ? error
    : new DaemonLoginSyncError(
        reason,
        error instanceof Error ? error.message : String(error),
      );
}

/**
 * Bring the local daemon up for a freshly signed-in user.
 *
 * The order matters and is the reason this is a function rather than three
 * inline awaits. Desktop's daemon profile is derived from the target API URL,
 * so until the main process has that URL there is no Desktop-owned profile —
 * and main now refuses to write the token at all rather than fall back to the
 * user's default CLI profile at `~/.patchbay/` (#6399).
 *
 * That refusal has no retry of its own: the login effect re-runs on `user`, not
 * on the target URL. So the URL must be pushed and awaited here, before the
 * token sync, instead of racing a separate effect's IPC message. Re-sending it
 * is cheap — the main-process handler ignores an unchanged value.
 */
export interface DaemonLoginSyncAPI {
  setTargetApiUrl: (url: string) => Promise<void>;
  syncToken: (token: string, userId: string) => Promise<void>;
  autoStart: () => Promise<DaemonAutoStartResult>;
  getStatus: () => Promise<DaemonStatus>;
  onStatusChange: (callback: (status: DaemonStatus) => void) => () => void;
}

function daemonStatusError(status: DaemonStatus): DaemonLoginSyncError {
  const reason: DaemonAutoStartFailureReason =
    status.state === "auth_expired"
      ? "auth_expired"
      : status.state === "cli_not_found"
        ? "cli_not_found"
        : "not_ready";
  return new DaemonLoginSyncError(
    reason,
    `daemon did not become ready (state: ${status.state})`,
  );
}

/**
 * Wait for the same Desktop-owned daemon that autoStart kicked off to report
 * readiness. The status event subscription is installed before the first
 * direct read, so a fast start cannot race between the two observations.
 */
export function waitForDaemonRunning(
  api: Pick<DaemonLoginSyncAPI, "getStatus" | "onStatusChange">,
  {
    timeoutMs = DAEMON_READY_TIMEOUT_MS,
    pollMs = DAEMON_READY_POLL_MS,
    expectedProfile,
  }: WaitForDaemonRunningOptions = {},
): Promise<DaemonStatus> {
  return new Promise((resolve, reject) => {
    let settled = false;
    let timer: ReturnType<typeof setTimeout> | undefined;
    let unsubscribe: () => void = () => undefined;
    let pollInFlight = false;

    const cleanup = () => {
      if (timer) clearTimeout(timer);
      if (timeout) clearTimeout(timeout);
      unsubscribe();
    };
    const finish = (callback: () => void) => {
      if (settled) return;
      settled = true;
      cleanup();
      callback();
    };
    const observe = (status: DaemonStatus) => {
      // Status events are only wake-ups. A delayed event from a previous
      // account/profile must not satisfy this login gate; the authoritative
      // getStatus() read below must belong to the profile we just started.
      if (
        expectedProfile &&
        status.profile &&
        status.profile !== expectedProfile
      ) {
        return false;
      }
      if (status.state === "running") {
        finish(() => resolve(status));
        return true;
      }
      if (
        status.state === "auth_expired" ||
        status.state === "cli_not_found" ||
        status.state === "stopped"
      ) {
        finish(() => reject(daemonStatusError(status)));
        return true;
      }
      return false;
    };

    const poll = async () => {
      if (settled || pollInFlight) return;
      pollInFlight = true;
      try {
        const status = await api.getStatus();
        observe(status);
      } catch (error) {
        finish(() => reject(asDaemonLoginSyncError(error)));
      } finally {
        pollInFlight = false;
        if (!settled) timer = setTimeout(() => void poll(), pollMs);
      }
    };

    const timeout = setTimeout(() => {
      finish(() =>
        reject(
          new DaemonLoginSyncError(
            "not_ready",
            `daemon did not become ready within ${Math.ceil(timeoutMs / 1000)}s; inspect the daemon log and retry`,
          ),
        ),
      );
    }, timeoutMs);

    // Subscribe first. Events are deliberately treated as a signal to re-read
    // state rather than as proof of readiness; getStatus() is scoped to the
    // active profile and closes the stale-event race.
    try {
      unsubscribe = api.onStatusChange(() => {
        if (timer) {
          clearTimeout(timer);
          timer = undefined;
        }
        void poll();
      });
    } catch (error) {
      finish(() => reject(asDaemonLoginSyncError(error)));
      return;
    }
    void poll();
  });
}

export async function syncDaemonOnLogin(
  api: DaemonLoginSyncAPI,
  apiUrl: string,
  token: string,
  userId: string,
): Promise<void> {
  await api.setTargetApiUrl(apiUrl);
  await api.syncToken(token, userId);
  const result = await api.autoStart();
  if (!result.success) {
    throw new DaemonLoginSyncError(result.reason, result.error);
  }
  await waitForDaemonRunning(api, { expectedProfile: result.profile });
}
