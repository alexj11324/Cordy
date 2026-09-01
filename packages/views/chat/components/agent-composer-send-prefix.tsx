"use client";

import { useCallback, useMemo } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import type { Agent, AgentRuntime } from "@patchbay/core/types";
import { api } from "@patchbay/core/api";
import { useWorkspaceId } from "@patchbay/core/hooks";
import { runtimeModelsOptions } from "@patchbay/core/runtimes";
import { cacheAgentResponse } from "@patchbay/core/workspace/queries";
import { ModelPicker } from "../../agents/components/inspector/model-picker";
import { ThinkingPicker } from "../../agents/components/inspector/thinking-picker";
import {
  buildModelChangeUpdate,
  type ModelCatalog,
} from "../../agents/components/inspector/model-change-cleanup";
import { findModelCapabilityEntry } from "../../agents/components/inspector/model-capability";

export function AgentComposerSendPrefix({
  agent,
  runtime,
  canEdit,
}: {
  agent: Agent;
  runtime: AgentRuntime | null;
  canEdit: boolean;
}) {
  const qc = useQueryClient();
  const wsId = useWorkspaceId();
  const isOnline = runtime?.status === "online";
  const modelsQuery = useQuery(
    runtimeModelsOptions(isOnline ? agent.runtime_id : null),
  );
  const catalog = useMemo<ModelCatalog>(
    () =>
      modelsQuery.isSuccess
        ? modelsQuery.data.supported
          ? modelsQuery.data.models
          : []
        : null,
    [modelsQuery.data, modelsQuery.isSuccess],
  );
  const levels = useMemo(() => {
    const entry = findModelCapabilityEntry(
      modelsQuery.data?.models ?? [],
      agent.model,
      runtime?.provider ?? "",
    );
    return entry?.thinking?.supported_levels ?? [];
  }, [agent.model, modelsQuery.data?.models, runtime?.provider]);

  const persist = useCallback(
    async (data: Record<string, unknown>) => {
      const updated = await api.updateAgent(agent.id, data);
      cacheAgentResponse(qc, wsId, updated, { insertIntoList: false });
    },
    [agent.id, qc, wsId],
  );

  return (
    <>
      <ModelPicker
        runtimeId={agent.runtime_id}
        runtimeOnline={isOnline}
        value={agent.model}
        canEdit={canEdit}
        showLabel={false}
        onChange={(model) =>
          persist(
            buildModelChangeUpdate({
              provider: runtime?.provider ?? "",
              model,
              thinkingLevel: agent.thinking_level ?? "",
              serviceTier: agent.service_tier ?? "",
              catalog,
            }),
          )
        }
      />
      {(levels.length > 0 || agent.thinking_level) && (
        <ThinkingPicker
          value={agent.thinking_level ?? ""}
          levels={levels}
          canEdit={canEdit}
          showLabel={false}
          onChange={(thinkingLevel) => persist({ thinking_level: thinkingLevel })}
        />
      )}
    </>
  );
}
