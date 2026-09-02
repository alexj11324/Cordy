import { spawn } from "node:child_process";

export const DESKTOP_PROFILE_HELPER_ARG =
  "--patchbay-private-desktop-profile";

export type DesktopProfileRequest =
  | { action: "configure"; profile: string; server_url: string }
  | {
      action: "set_credentials";
      profile: string;
      server_url: string;
      token: string;
      user_id: string;
    }
  | { action: "clear_credentials"; profile: string };

/**
 * Ask the bundled Go CLI to mutate a Desktop profile under its native
 * cross-process lock. The request travels over stdin so credentials never
 * appear in argv, process listings, or launcher logs.
 */
export function runDesktopProfileHelper(
  binary: string,
  request: DesktopProfileRequest,
  env: NodeJS.ProcessEnv,
  timeoutMs = 15_000,
): Promise<void> {
  return new Promise((resolve, reject) => {
    const child = spawn(binary, [DESKTOP_PROFILE_HELPER_ARG], {
      env,
      stdio: ["pipe", "ignore", "pipe"],
      windowsHide: true,
    });
    let stderr = "";
    let settled = false;
    let timedOut = false;
    let transportError: Error | undefined;
    let escalationTimer: ReturnType<typeof setTimeout> | undefined;
    const timer = setTimeout(() => {
      timedOut = true;
      child.kill();
      escalationTimer = setTimeout(() => child.kill("SIGKILL"), 1_000);
    }, timeoutMs);

    const finish = (error?: Error) => {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      if (escalationTimer) clearTimeout(escalationTimer);
      if (error) reject(error);
      else resolve();
    };

    child.stderr.on("data", (chunk: Buffer) => {
      if (stderr.length < 8_192) stderr += chunk.toString("utf-8");
    });
    child.on("error", (error) => finish(error));
    child.on("close", (code, signal) => {
      if (timedOut) {
        finish(new Error("Desktop profile helper timed out"));
        return;
      }
      if (transportError) {
        finish(transportError);
        return;
      }
      if (code === 0) {
        finish();
        return;
      }
      const detail = stderr.trim().slice(0, 1_024);
      finish(
        new Error(
          `Desktop profile helper failed (${signal ?? `exit ${code ?? "unknown"}`})${detail ? `: ${detail}` : ""}`,
        ),
      );
    });
    child.stdin.on("error", (error) => {
      transportError = error;
      child.kill();
    });
    child.stdin.end(`${JSON.stringify(request)}\n`);
  });
}
