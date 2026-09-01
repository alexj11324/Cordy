"use client";

import { useCallback } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { FolderGit2, Monitor } from "lucide-react";
import type { Agent, AgentRuntime } from "@patchbay/core/types";
import { api } from "@patchbay/core/api";
import { useWorkspaceId } from "@patchbay/core/hooks";
import { useCurrentWorkspace } from "@patchbay/core/paths";
import { runtimeDisplayName, runtimeModelsOptions } from "@patchbay/core/runtimes";
import { cacheAgentResponse } from "@patchbay/core/workspace/queries";
import { githubShortLabel } from "../../common/github-url";
import { useT } from "../../i18n";
import { SessionModePicker } from "./session-mode-picker";

export function AgentComposerControlBar({
  agent,
  runtime,
  canEdit,
}: {
  agent: Agent | null;
  runtime: AgentRuntime | null;
  canEdit: boolean;
}) {
  const { t } = useT("chat");
  const qc = useQueryClient();
  const wsId = useWorkspaceId();
  const workspace = useCurrentWorkspace();
  const isOnline = runtime?.status === "online";
  const modelsQuery = useQuery(
    runtimeModelsOptions(isOnline && agent ? agent.runtime_id : null),
  );
  const repoUrl = workspace?.repos[0]?.url?.trim() ?? "";
  const repoLabel = repoUrl ? githubShortLabel(repoUrl) : "";
  const deviceLabel = runtime
    ? runtimeDisplayName(runtime)
    : t(($) => $.control_bar.unknown_device);

  const persistMode = useCallback(
    async (sessionMode: string) => {
      if (!agent) return;
      const updated = await api.updateAgent(agent.id, { session_mode: sessionMode });
      cacheAgentResponse(qc, wsId, updated, { insertIntoList: false });
    },
    [agent, qc, wsId],
  );

  return (
    <div className="flex h-7 items-center justify-between gap-2 px-1 text-caption text-muted-foreground">
      <div className="flex min-w-0 items-center gap-2">
        <span className="inline-flex min-w-0 items-center gap-1 truncate" title={deviceLabel}>
          <Monitor className="h-3.5 w-3.5 shrink-0" aria-hidden="true" />
          <span className="truncate">{deviceLabel}</span>
        </span>
        {repoLabel ? (
          <span className="inline-flex min-w-0 items-center gap-1 truncate" title={repoLabel}>
            <FolderGit2 className="h-3.5 w-3.5 shrink-0" aria-hidden="true" />
            <span className="truncate">{repoLabel}</span>
          </span>
        ) : null}
      </div>
      <SessionModePicker
        value={agent?.session_mode ?? ""}
        advertised={modelsQuery.data?.session_modes}
        canEdit={canEdit && !!agent}
        onChange={persistMode}
      />
    </div>
  );
}
