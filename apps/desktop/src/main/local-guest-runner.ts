import { app, BrowserWindow, ipcMain } from "electron";
import { randomUUID } from "node:crypto";
import { spawn, type ChildProcessWithoutNullStreams } from "node:child_process";
import { createInterface } from "node:readline";
import type {
  LocalGuestMode,
  LocalGuestRunCancelResult,
  LocalGuestRunEvent,
  LocalGuestRunHistory,
  LocalGuestRunHistoryResult,
  LocalGuestRunRequest,
  LocalGuestRunStartResult,
} from "../shared/local-guest";
import {
  MAX_LOCAL_GUEST_OUTPUT_LENGTH,
  parseLocalGuestRunEvent,
  parseLocalGuestRunHistory,
  parseLocalGuestRunRequest,
} from "../shared/local-guest";
import { validateLocalDirectory } from "./local-directory";
import {
  bundledCliPath,
  localGuestChildEnvironment,
  verifyBundledCli,
} from "./local-guest-runtime";
import {
  clearLocalGuestRunHistory,
  loadLocalGuestRunHistory,
  localGuestRunHistoryPath,
  saveLocalGuestRunHistory,
} from "./local-guest-history-storage";
import type { LocalWorkspaceGrants } from "./local-guest-workspace";

type MainWindowGetter = () => BrowserWindow | null;
type ModeGetter = () => LocalGuestMode;

type ActiveRun = {
  id: string;
  child: ChildProcessWithoutNullStreams;
  request: LocalGuestRunRequest;
  /** The realpath the grant check resolved to — never the renderer's string. */
  workingDirectory: string;
  startedAt: number;
  output: string;
  result: LocalGuestRunEvent | null;
  cancelReason: "cancelled" | "timeout" | null;
  protocolError: string | null;
  stderr: string;
  timeout: ReturnType<typeof setTimeout>;
  historySaved: boolean;
};

function isMainWindowSender(
  event: Electron.IpcMainInvokeEvent,
  getMainWindow: MainWindowGetter,
): boolean {
  const senderWindow = BrowserWindow.fromWebContents(event.sender);
  const mainWindow = getMainWindow();
  return Boolean(
    senderWindow &&
      mainWindow &&
      senderWindow === mainWindow &&
      !senderWindow.isDestroyed(),
  );
}

function sendRunEvent(
  getMainWindow: MainWindowGetter,
  runId: string,
  event: LocalGuestRunEvent,
): void {
  const window = getMainWindow();
  if (!window || window.isDestroyed() || window.webContents.isDestroyed()) return;
  try {
    window.webContents.send("guest-run:event", { runId, ...event });
  } catch {
    // The window may close while a child is flushing its final JSON line.
  }
}

function appendBounded(current: string, next: string): string {
  if (!next || current.length >= MAX_LOCAL_GUEST_OUTPUT_LENGTH) return current;
  const remaining = MAX_LOCAL_GUEST_OUTPUT_LENGTH - current.length;
  return current + next.slice(0, remaining);
}

function parseChildEvent(line: string): LocalGuestRunEvent | null {
  try {
    return parseLocalGuestRunEvent(JSON.parse(line) as unknown);
  } catch {
    return null;
  }
}

function historyWithNewRun(
  history: LocalGuestRunHistory,
  run: ActiveRun,
  result: LocalGuestRunEvent,
): LocalGuestRunHistory {
  const entry = {
    id: run.id,
    prompt: run.request.prompt,
    workingDirectory: run.workingDirectory,
    status: result.status ?? "failed",
    output: run.output,
    ...(result.error ? { error: result.error } : {}),
    startedAt: run.startedAt,
    ...(result.durationMs != null
      ? { durationMs: result.durationMs }
      : { durationMs: Date.now() - run.startedAt }),
  };
  return {
    lastDirectory: run.workingDirectory,
    runs: [entry, ...history.runs].slice(0, 20),
  };
}

export function setupLocalGuestRunner(
  getMainWindow: MainWindowGetter,
  getMode: ModeGetter,
  grants: LocalWorkspaceGrants,
): { clear: () => Promise<void>; cancel: () => void } {
  const historyPath = localGuestRunHistoryPath(app.getPath("userData"));
  let activeRun: ActiveRun | null = null;

  // `lastDirectory` was granted through the OS picker in an earlier session
  // and is handed straight back to the renderer as a prefill, so re-granting
  // it here is what makes that prefill runnable without re-picking. It is read
  // from main-owned 0600 state, never from the renderer.
  const seededGrants = loadLocalGuestRunHistory(historyPath)
    .then(async (result) => {
      if (result.ok && result.history.lastDirectory) {
        await grants.grant(result.history.lastDirectory);
      }
    })
    .catch(() => undefined);

  const cancel = (reason: "cancelled" | "timeout" = "cancelled"): void => {
    const run = activeRun;
    if (!run || run.cancelReason) return;
    run.cancelReason = reason;
    run.child.kill("SIGTERM");
    setTimeout(() => {
      if (activeRun === run && !run.child.killed) run.child.kill("SIGKILL");
    }, 2_000);
  };

  const persistCompletedRun = async (
    run: ActiveRun,
    result: LocalGuestRunEvent,
  ): Promise<void> => {
    if (run.historySaved) return;
    run.historySaved = true;
    const existing = await loadLocalGuestRunHistory(historyPath);
    if (!existing.ok) throw new Error("Guest run history is unavailable");
    await saveLocalGuestRunHistory(
      historyPath,
      historyWithNewRun(existing.history, run, result),
    );
  };

  const start = async (
    event: Electron.IpcMainInvokeEvent,
    value: unknown,
  ): Promise<LocalGuestRunStartResult> => {
    if (!isMainWindowSender(event, getMainWindow)) {
      return { ok: false, reason: "unauthorized" };
    }
    if (getMode() !== "guest") return { ok: false, reason: "guest_required" };
    if (activeRun) return { ok: false, reason: "busy" };
    const request = parseLocalGuestRunRequest(value);
    if (!request) return { ok: false, reason: "invalid_request" };

    // Consent first, then shape. A path the user never chose is rejected even
    // when it is a perfectly valid readable directory.
    await seededGrants;
    const workingDirectory = await grants.resolveGranted(
      request.workingDirectory,
    );
    if (!workingDirectory) return { ok: false, reason: "invalid_directory" };
    const directory = await validateLocalDirectory(workingDirectory);
    if (!directory.ok) return { ok: false, reason: "invalid_directory" };

    const binaryPath = bundledCliPath(app.getAppPath());
    if (!(await verifyBundledCli(binaryPath))) {
      return { ok: false, reason: "cli_unavailable" };
    }
    const history = await loadLocalGuestRunHistory(historyPath);
    if (!history.ok) return { ok: false, reason: "unavailable" };

    const child = spawn(binaryPath, ["daemon", "run-local"], {
      cwd: workingDirectory,
      env: await localGuestChildEnvironment(),
      stdio: ["pipe", "pipe", "pipe"],
      windowsHide: true,
    });
    const run: ActiveRun = {
      id: randomUUID(),
      child,
      request,
      workingDirectory,
      startedAt: Date.now(),
      output: "",
      result: null,
      cancelReason: null,
      protocolError: null,
      stderr: "",
      timeout: setTimeout(() => cancel("timeout"), request.timeoutMs),
      historySaved: false,
    };
    activeRun = run;

    const finish = async (eventFromChild?: LocalGuestRunEvent) => {
      if (activeRun !== run) return;
      clearTimeout(run.timeout);
      let result = eventFromChild ?? run.result;
      if (!result || result.event !== "result") {
        result = {
          event: "result",
          status: run.cancelReason ?? "failed",
          ...(run.protocolError
            ? { error: run.protocolError }
            : run.stderr
              ? { error: run.stderr.slice(-4_096) }
              : { error: "Local runner exited without a result" }),
          durationMs: Date.now() - run.startedAt,
        };
      }
      if (run.cancelReason) {
        result = {
          ...result,
          status: run.cancelReason,
          ...(run.cancelReason === "timeout"
            ? { error: "Local run timed out" }
            : { error: "Local run cancelled" }),
        };
      }
      if (result.status === "completed" && run.protocolError) {
        result = {
          ...result,
          status: "failed",
          error: run.protocolError,
        };
      }
      if (!run.result || run.result.status !== result.status) {
        sendRunEvent(getMainWindow, run.id, result);
      }
      try {
        await persistCompletedRun(run, result);
      } catch {
        // The run result remains visible. A later history read will report the
        // unavailable store rather than pretending persistence succeeded.
      }
      if (activeRun === run) activeRun = null;
    };

    const lines = createInterface({ input: child.stdout });
    lines.on("line", (line) => {
      const childEvent = parseChildEvent(line);
      if (!childEvent) {
        run.protocolError = "Local runner returned invalid JSON";
        cancel();
        return;
      }
      if (childEvent.text) run.output = appendBounded(run.output, childEvent.text + "\n");
      if (childEvent.error) run.output = appendBounded(run.output, childEvent.error + "\n");
      if (childEvent.event === "result") run.result = childEvent;
      sendRunEvent(getMainWindow, run.id, childEvent);
    });
    child.stderr.on("data", (chunk: Buffer | string) => {
      run.stderr = appendBounded(run.stderr, String(chunk));
    });
    child.once("error", (error) => {
      run.protocolError = error.message;
      void finish();
    });
    child.once("close", () => {
      lines.close();
      void finish();
    });

    child.stdin.once("error", (error) => {
      run.protocolError = error.message;
    });
    child.stdin.end(
      JSON.stringify({
        working_directory: workingDirectory,
        prompt: request.prompt,
        timeout_ms: request.timeoutMs,
      }) + "\n",
    );
    return { ok: true, runId: run.id };
  };

  ipcMain.handle("guest-run:start", start);
  ipcMain.handle(
    "guest-run:cancel",
    (event, value: unknown): LocalGuestRunCancelResult => {
      if (!isMainWindowSender(event, getMainWindow)) {
        return { ok: false, reason: "unauthorized" };
      }
      if (getMode() !== "guest") return { ok: false, reason: "guest_required" };
      if (
        !value ||
        typeof value !== "object" ||
        Array.isArray(value) ||
        typeof (value as { runId?: unknown }).runId !== "string" ||
        activeRun?.id !== (value as { runId: string }).runId
      ) {
        return { ok: false, reason: "not_found" };
      }
      cancel();
      return { ok: true };
    },
  );
  ipcMain.handle(
    "guest-run:history",
    async (event): Promise<LocalGuestRunHistoryResult> => {
      if (!isMainWindowSender(event, getMainWindow)) {
        return { ok: false, reason: "unauthorized" };
      }
      if (getMode() !== "guest") return { ok: false, reason: "guest_required" };
      const result = await loadLocalGuestRunHistory(historyPath);
      if (!result.ok || !parseLocalGuestRunHistory(result.history)) {
        return { ok: false, reason: "unavailable" };
      }
      return { ok: true, history: result.history };
    },
  );

  ipcMain.handle("guest-run:clear-history", async (event) => {
    if (!isMainWindowSender(event, getMainWindow)) return { ok: false };
    if (getMode() !== "guest") return { ok: false };
    await clearLocalGuestRunHistory(historyPath);
    return { ok: true };
  });

  return {
    cancel: () => cancel(),
    clear: async () => {
      cancel();
      await clearLocalGuestRunHistory(historyPath);
    },
  };
}

