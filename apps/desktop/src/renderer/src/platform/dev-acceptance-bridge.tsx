import { useEffect, useRef } from "react";
import { api } from "@patchbay/core/api";
import { useCurrentWorkspace, paths } from "@patchbay/core/paths";
import { setCurrentWorkspace } from "@patchbay/core/platform";
import type { AgentRuntime, AgentTask, Workspace } from "@patchbay/core/types";
import { useNavigation } from "@patchbay/views/navigation";
import { findActiveHubInstallation } from "./dev-acceptance-provider";

const ACCEPTANCE_ENABLED =
  import.meta.env.DEV &&
  import.meta.env.VITE_PATCHBAY_DEV_ACCEPTANCE === "1" &&
  window.desktopAPI.host === "electron" &&
  window.desktopAPI.windowContext?.kind === "main";

const DEFAULT_TIMEOUT_MS = 5 * 60 * 1000;
const POLL_INTERVAL_MS = 1_000;
const TERMINAL_TASK_STATUSES = new Set(["completed", "failed", "cancelled"]);

export type DevAcceptanceProvider = "telegram" | "weixin";

type DevAcceptanceProviderResult = {
  kind: DevAcceptanceProvider;
  installationId: string;
  installedAt: string;
  roundTripStatus: string;
};

export type DevAcceptancePhase =
  | "idle"
  | "checking-daemon"
  | "checking-runtimes"
  | "creating-agent"
  | "creating-issue"
  | "issue-open"
  | "waiting-task"
  | "verifying-provider"
  | "passed"
  | "failed"
  | "cleaning"
  | "complete";

export type DevAcceptanceResult =
  | {
      ok: true;
      runId: string;
      marker: string;
      workspaceSlug: string;
      issueId: string;
      taskId: string;
      agentId: string;
      provider?: DevAcceptanceProviderResult;
    }
  | {
      ok: false;
      runId: string;
      code: string;
      message: string;
      fix: string;
    };

export type DevAcceptanceStatus = {
  runId: string | null;
  phase: DevAcceptancePhase;
  marker?: string;
  workspaceSlug?: string;
  issueId?: string;
  agentId?: string;
  taskId?: string;
  message?: string;
  fix?: string;
  result?: DevAcceptanceResult;
};

export type DevAcceptanceStartOptions = {
  marker?: string;
  provider?: DevAcceptanceProvider;
  runtimeProvider?: string;
  timeoutMs?: number;
};

export type DevAcceptanceCleanupResult = {
  ok: boolean;
  leftovers: string[];
  message?: string;
};

export type DevAcceptanceHook = {
  start(options?: DevAcceptanceStartOptions):
    | { started: true; runId: string; marker: string }
    | { started: false; message: string };
  getStatus(): DevAcceptanceStatus;
  cleanup(): Promise<DevAcceptanceCleanupResult>;
};

declare global {
  interface Window {
    /** Installed only by the explicit credentialed dev acceptance launcher. */
    __PATCHBAY_DEV_ACCEPTANCE__?: DevAcceptanceHook;
  }
}

class AcceptanceFailure extends Error {
  readonly code: string;
  readonly fix: string;

  constructor(code: string, message: string, fix: string) {
    super(message);
    this.name = "AcceptanceFailure";
    this.code = code;
    this.fix = fix;
  }
}

function safeErrorMessage(error: unknown): string {
  const raw = error instanceof Error ? error.message : String(error);
  return raw
    .replace(/Bearer\s+[^\s)]+/gi, "Bearer [redacted]")
    .replace(/(token|secret|password|bot[_-]?token)\s*[:=]\s*[^\s,;)}]+/gi, "$1=[redacted]")
    .slice(0, 500);
}

function normalizeUrl(value: string | undefined): string | null {
  if (!value) return null;
  try {
    const url = new URL(value);
    url.pathname = url.pathname.replace(/\/+$/, "");
    return url.toString().replace(/\/+$/, "");
  } catch {
    return value.replace(/\/+$/, "");
  }
}

function assertMarker(value: string | undefined): string {
  const marker = value?.trim() || `PATCHBAY_DEV_ACCEPTANCE_${Date.now()}`;
  if (!/^[A-Za-z0-9][A-Za-z0-9_-]{7,127}$/.test(marker)) {
    throw new AcceptanceFailure(
      "invalid_marker",
      "the acceptance marker must contain only letters, numbers, `_` or `-` and be 8–128 characters",
      "Pass a stable marker such as `PATCHBAY_DEV_ACCEPTANCE_20260831`.",
    );
  }
  return marker;
}

function assertTimeout(value: number | undefined): number {
  const timeout = value ?? DEFAULT_TIMEOUT_MS;
  if (!Number.isInteger(timeout) || timeout < 10_000 || timeout > 15 * 60 * 1000) {
    throw new AcceptanceFailure(
      "invalid_timeout",
      "the acceptance timeout must be an integer between 10000 and 900000 milliseconds",
      "Pass a timeout in the supported range, or omit it to use five minutes.",
    );
  }
  return timeout;
}

function taskText(task: AgentTask, messages: readonly { content?: string; output?: string }[]): string {
  const result = task.result;
  const resultText =
    typeof result === "string"
      ? result
      : result && typeof result === "object"
        ? Object.values(result as Record<string, unknown>)
            .filter((value): value is string => typeof value === "string")
            .join("\n")
        : "";
  return [
    resultText,
    task.error ?? "",
    ...messages.flatMap((message) => [message.content ?? "", message.output ?? ""]),
  ].join("\n");
}

function chooseRuntime(
  runtimes: readonly AgentRuntime[],
  daemonId: string,
  runtimeProvider?: string,
): AgentRuntime {
  const online = runtimes.filter(
    (runtime) => runtime.status === "online" && runtime.daemon_id === daemonId,
  );
  const candidates = runtimeProvider
    ? online.filter(
        (runtime) => runtime.provider.toLowerCase() === runtimeProvider,
      )
    : online;
  if (candidates.length === 0) {
    throw new AcceptanceFailure(
      "runtime_unavailable",
      runtimeProvider
        ? `no online ${runtimeProvider} runtime is registered to the running Desktop daemon`
        : "no online runtime is registered to the running Desktop daemon",
      "Open Desktop Runtimes, start the source-matched daemon, and install or sign in to the local agent CLI; then rerun the acceptance command. If more than one runtime is online, pass its CLI provider with `--runtime-provider`.",
    );
  }
  if (candidates.length > 1 && !runtimeProvider) {
    throw new AcceptanceFailure(
      "runtime_ambiguous",
      `found ${candidates.length} online runtimes for this daemon; refusing to choose one implicitly`,
      "Pass the intended CLI provider explicitly, for example `--runtime-provider codex`, or leave exactly one local runtime online. Integration checks use a separate `--provider telegram|weixin` option.",
    );
  }
  return candidates[0]!;
}

async function verifyProvider(
  workspaceId: string,
  provider: DevAcceptanceProvider,
): Promise<DevAcceptanceProviderResult> {
  const response =
    provider === "telegram"
      ? await api.listTelegramInstallations(workspaceId)
      : await api.listWeixinInstallations(workspaceId);
  if (!response.configured) {
    throw new AcceptanceFailure(
      `${provider}_not_configured`,
      `${provider} is not enabled on this development backend`,
      provider === "telegram"
        ? "Set PATCHBAY_TELEGRAM_SECRET_KEY through the existing development secret mechanism, restart `pnpm dev`, and register a BotFather token in Settings → Integrations."
        : "Set PATCHBAY_WEIXIN_SECRET_KEY through the existing development secret mechanism, restart `pnpm dev`, and finish the iLink QR flow in Settings → Integrations.",
    );
  }
  const installation = findActiveHubInstallation(response.installations);
  if (!installation) {
    throw new AcceptanceFailure(
      `${provider}_installation_missing`,
      `no active ${provider} installation exists for this workspace`,
      provider === "telegram"
        ? "In the same Electron window, open Settings → Integrations → Telegram and complete the BotFather connection."
        : "In the same Electron window, open Settings → Integrations → WeChat and complete the QR authorization.",
    );
  }
  if (installation.round_trip_status !== "passed") {
    throw new AcceptanceFailure(
      `${provider}_round_trip_missing`,
      `the active ${provider} installation has not recorded a successful message round trip`,
      provider === "telegram"
        ? "Send a real message to the connected Telegram bot and wait for the installation status to become Verified; this command never treats an active credential as a successful test."
        : "Send a real message to the connected Weixin account and wait for the installation status to become Verified; this command never treats an active credential as a successful test.",
    );
  }
  return {
    kind: provider,
    installationId: installation.id,
    installedAt: installation.installed_at,
    roundTripStatus: installation.round_trip_status,
  };
}

function createStatusStore() {
  let status: DevAcceptanceStatus = { runId: null, phase: "idle" };
  return {
    get: () => ({ ...status }),
    set: (next: Partial<DevAcceptanceStatus>) => {
      status = { ...status, ...next };
    },
  };
}

/**
 * Mounts the acceptance control plane inside the authenticated main renderer.
 * It is deliberately not a UI fallback and is never present in normal dev or
 * packaged builds. The Node runner invokes this narrow hook through a
 * loopback-only CDP connection, so all API calls retain the renderer's normal
 * auth, workspace headers, schemas, and realtime state.
 */
export function DevAcceptanceBridge() {
  const workspace = useCurrentWorkspace();
  const navigation = useNavigation();
  const workspaceRef = useRef<Workspace | null>(workspace);
  const navigationRef = useRef(navigation);
  const statusStoreRef = useRef(createStatusStore());
  const runPromiseRef = useRef<Promise<DevAcceptanceResult> | null>(null);
  const resourcesRef = useRef<{
    issueId: string;
    agentId: string;
  } | null>(null);
  const cleanupPromiseRef = useRef<Promise<DevAcceptanceCleanupResult> | null>(null);

  useEffect(() => {
    workspaceRef.current = workspace;
  }, [workspace]);
  useEffect(() => {
    navigationRef.current = navigation;
  }, [navigation]);

  useEffect(() => {
    if (!ACCEPTANCE_ENABLED) return undefined;

    const statusStore = statusStoreRef.current;

    const run = async (
      runId: string,
      marker: string,
      provider: DevAcceptanceProvider | undefined,
      runtimeProvider: string | undefined,
      timeoutMs: number,
    ): Promise<DevAcceptanceResult> => {
      const startedAt = Date.now();
      const ensureWithinDeadline = () => {
        if (Date.now() - startedAt >= timeoutMs) {
          throw new AcceptanceFailure(
            "timeout",
            `acceptance timed out after ${timeoutMs}ms`,
            "Inspect the daemon log and backend task status, then rerun after the source-matched daemon is healthy.",
          );
        }
      };

      try {
        statusStore.set({ phase: "checking-daemon", message: "checking the Electron-owned daemon" });
        const daemon = await window.daemonAPI.getStatus();
        if (daemon.state !== "running" || !daemon.daemonId) {
          throw new AcceptanceFailure(
            "daemon_not_ready",
            `Electron daemon is not ready (state: ${daemon.state})`,
            "Sign in to Desktop, wait for the daemon status to become Running, and rerun the acceptance command.",
          );
        }
        const runtimeConfig = window.desktopAPI.runtimeConfig;
        const configuredApiUrl = runtimeConfig.ok ? runtimeConfig.config.apiUrl : null;
        if (!runtimeConfig.ok || !configuredApiUrl) {
          throw new AcceptanceFailure(
            "runtime_config_missing",
            "Electron has no validated API runtime configuration",
            "Fix ~/.patchbay/desktop.json or rerun the complete `pnpm dev` launcher so the backend URL is explicit.",
          );
        }
        if (
          daemon.serverUrl &&
          normalizeUrl(daemon.serverUrl) !== normalizeUrl(configuredApiUrl)
        ) {
          throw new AcceptanceFailure(
            "daemon_target_mismatch",
            "the running daemon is connected to a different backend than this Electron window",
            "Stop the stale Desktop daemon, rerun `pnpm dev`, and wait for the daemon to report the current backend URL.",
          );
        }
        const probe = await window.daemonAPI.probeRuntimes();
        if (probe.probeResult !== "success" || probe.runtimeCount < 1) {
          throw new AcceptanceFailure(
            "runtime_probe_failed",
            "the Electron daemon could not discover a local agent runtime",
            "Install/sign in to a supported local agent CLI, restart the complete dev stack, and rerun `pnpm dev:doctor`.",
          );
        }
        ensureWithinDeadline();

        const currentWorkspace = workspaceRef.current;
        if (!currentWorkspace) {
          throw new AcceptanceFailure(
            "workspace_not_ready",
            "the authenticated Electron window has no resolved workspace",
            "Finish onboarding or open a workspace in the main Electron window before starting acceptance.",
          );
        }
        setCurrentWorkspace(currentWorkspace.slug, currentWorkspace.id);

        statusStore.set({ phase: "checking-runtimes", workspaceSlug: currentWorkspace.slug });
        const runtimes = await api.listRuntimes(
          { workspace_id: currentWorkspace.id, owner: "me" },
          currentWorkspace.slug,
        );
        const runtime = chooseRuntime(
          runtimes,
          daemon.daemonId,
          runtimeProvider,
        );

        statusStore.set({ phase: "creating-agent", workspaceSlug: currentWorkspace.slug });
        const agent = await api.createAgent({
          name: `${marker} agent`,
          description: "Disposable agent created by the complete Electron development acceptance runner.",
          instructions: `Reply with the exact marker ${marker} as your first and only sentence. Do not modify files or call tools.`,
          runtime_id: runtime.id,
          permission_mode: "private",
        });
        resourcesRef.current = { agentId: agent.id, issueId: "" };
        statusStore.set({ agentId: agent.id });

        statusStore.set({ phase: "creating-issue" });
        const issue = await api.createIssue({
          title: `${marker} Electron development acceptance`,
          description: `This disposable issue verifies the real Electron → managed daemon → backend → agent path. Reply with the exact marker ${marker} and do not modify files.`,
          status: "in_progress",
          executor_type: "agent",
          executor_id: agent.id,
        });
        resourcesRef.current = { agentId: agent.id, issueId: issue.id };
        statusStore.set({ phase: "issue-open", issueId: issue.id, workspaceSlug: currentWorkspace.slug });
        navigationRef.current.push(
          paths.workspace(currentWorkspace.slug).issueDetail(issue.identifier || issue.id),
        );

        statusStore.set({ phase: "waiting-task", message: "waiting for the agent task and reply" });
        let completedTask: AgentTask | undefined;
        while (!completedTask) {
          ensureWithinDeadline();
          const tasks = await api.listTasksByIssue(issue.id);
          const task = tasks
            .filter((candidate) => candidate.agent_id === agent.id)
            .toSorted(
              (left, right) =>
                new Date(right.created_at).getTime() - new Date(left.created_at).getTime(),
            )[0];
          if (!task) {
            await new Promise((resolve) => setTimeout(resolve, POLL_INTERVAL_MS));
            continue;
          }
          statusStore.set({ taskId: task.id, message: `agent task is ${task.status}` });
          if (!TERMINAL_TASK_STATUSES.has(task.status)) {
            await new Promise((resolve) => setTimeout(resolve, POLL_INTERVAL_MS));
            continue;
          }
          if (task.status !== "completed") {
            throw new AcceptanceFailure(
              "agent_task_failed",
              `agent task ended with status ${task.status}${task.error ? `: ${safeErrorMessage(task.error)}` : ""}`,
              "Inspect the daemon log and the task's failure reason, fix the provider/CLI login or runtime, and rerun the acceptance command.",
            );
          }
          const messages = await api.listTaskMessages(task.id);
          if (!taskText(task, messages).includes(marker)) {
            throw new AcceptanceFailure(
              "agent_reply_marker_missing",
              "the agent task completed but did not return the acceptance marker",
              "Confirm that the selected local CLI is logged in and that the agent runtime can produce a final response; then rerun the acceptance command.",
            );
          }
          completedTask = task;
        }

        let verifiedProvider: DevAcceptanceProviderResult | undefined;
        if (provider) {
          statusStore.set({ phase: "verifying-provider", message: `checking ${provider} message verification` });
          verifiedProvider = await verifyProvider(currentWorkspace.id, provider);
        }

        const result: DevAcceptanceResult = {
          ok: true,
          runId,
          marker,
          workspaceSlug: currentWorkspace.slug,
          issueId: issue.id,
          taskId: completedTask.id,
          agentId: agent.id,
          ...(verifiedProvider ? { provider: verifiedProvider } : {}),
        };
        statusStore.set({ phase: "passed", result, message: "API round trip passed; waiting for the Electron conversation DOM check" });
        return result;
      } catch (error) {
        const failure =
          error instanceof AcceptanceFailure
            ? error
            : new AcceptanceFailure(
                "acceptance_error",
                safeErrorMessage(error),
                "Inspect the complete dev launcher, daemon, and backend logs, then rerun the acceptance command.",
              );
        const result: DevAcceptanceResult = {
          ok: false,
          runId,
          code: failure.code,
          message: failure.message,
          fix: failure.fix,
        };
        statusStore.set({ phase: "failed", result, message: failure.message, fix: failure.fix });
        return result;
      }
    };

    const cleanup = async (): Promise<DevAcceptanceCleanupResult> => {
      if (cleanupPromiseRef.current) return cleanupPromiseRef.current;
      const resources = resourcesRef.current;
      if (!resources || (!resources.issueId && !resources.agentId)) {
        statusStore.set({ phase: "complete", message: "no disposable resources were created" });
        return { ok: true, leftovers: [] };
      }
      cleanupPromiseRef.current = (async () => {
        statusStore.set({ phase: "cleaning", message: "cleaning disposable issue and agent" });
        const failures: string[] = [];
        if (resources.agentId) {
          try {
            await api.cancelAgentTasks(resources.agentId);
          } catch (error) {
            failures.push(`agent tasks (${safeErrorMessage(error)})`);
          }
        }
        if (resources.issueId) {
          try {
            await api.deleteIssue(resources.issueId);
          } catch (error) {
            failures.push(`issue ${resources.issueId} (${safeErrorMessage(error)})`);
          }
        }
        if (resources.agentId) {
          try {
            await api.archiveAgent(resources.agentId);
          } catch (error) {
            failures.push(`agent ${resources.agentId} (${safeErrorMessage(error)})`);
          }
        }
        const result = {
          ok: failures.length === 0,
          leftovers: failures.map((failure) => failure.split(" (")[0]!),
          ...(failures.length > 0
            ? { message: `acceptance passed, but cleanup failed: ${failures.join("; ")}` }
            : {}),
        };
        statusStore.set({
          phase: result.ok ? "complete" : "failed",
          message: result.message ?? "acceptance resources cleaned up",
        });
        return result;
      })();
      return cleanupPromiseRef.current;
    };

    const hook: DevAcceptanceHook = {
      start: (options = {}) => {
        if (runPromiseRef.current || cleanupPromiseRef.current || statusStore.get().phase === "complete") {
          return { started: false, message: "an Electron development acceptance run is already active" };
        }
        let marker: string;
        let timeoutMs: number;
        try {
          marker = assertMarker(options.marker);
          timeoutMs = assertTimeout(options.timeoutMs);
        } catch (error) {
          const message = safeErrorMessage(error);
          return { started: false, message };
        }
        const runId = `${marker}-${Date.now()}`;
        statusStore.set({ runId, phase: "idle", marker, message: "starting acceptance" });
        runPromiseRef.current = run(
          runId,
          marker,
          options.provider,
          options.runtimeProvider,
          timeoutMs,
        ).finally(() => {
          runPromiseRef.current = null;
        });
        return { started: true, runId, marker };
      },
      getStatus: () => statusStore.get(),
      cleanup,
    };
    window.__PATCHBAY_DEV_ACCEPTANCE__ = hook;
    return () => {
      if (window.__PATCHBAY_DEV_ACCEPTANCE__ === hook) {
        delete window.__PATCHBAY_DEV_ACCEPTANCE__;
      }
    };
  }, []);

  return null;
}
