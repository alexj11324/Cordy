"use client";

import { useCallback, useEffect, useMemo, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import type { Agent, AgentRuntime, MemberWithUser } from "@orvilo/core/types";
import {
  AGENT_MAX_CONCURRENT_TASKS_MAX,
  AGENT_MAX_CONCURRENT_TASKS_MIN,
} from "@orvilo/core/agents";
import {
  isRuntimeUsableForUser,
  runtimeModelsOptions,
} from "@orvilo/core/runtimes";
import { isImeComposing } from "@orvilo/core/utils";
import { Input } from "@orvilo/ui/components/ui/input";
import {
  SettingsCard,
  SettingsRow,
  SettingsSection,
} from "../../settings/components/settings-layout";
import { useT } from "../../i18n";
import { ModelPicker } from "./inspector/model-picker";
import {
  buildModelChangeUpdate,
  type ModelCatalog,
} from "./inspector/model-change-cleanup";
import { findModelCapabilityEntry } from "./inspector/model-capability";

interface InspectorProps {
  agent: Agent;
  runtime: AgentRuntime | null;
  runtimes: AgentRuntime[];
  members: MemberWithUser[];
  currentUserId: string | null;
  canEdit: boolean;
  onUpdate: (id: string, data: Record<string, unknown>) => Promise<void>;
}

/**
 * Full-width General settings form. Identity (name/avatar) lives on the
 * page card; this surface is how the agent runs.
 */
export function AgentDetailInspector({
  agent,
  runtime,
  runtimes,
  currentUserId,
  canEdit,
  onUpdate,
}: InspectorProps) {
  const { t } = useT("agents");
  const update = useCallback(
    (data: Record<string, unknown>) => onUpdate(agent.id, data),
    [agent.id, onUpdate],
  );

  const isOnline = runtime?.status === "online";
  const canReadRuntime =
    runtime != null && isRuntimeUsableForUser(runtime, currentUserId);
  const canDiscoverRuntimeModels = isOnline && canReadRuntime;

  // Same query the Thinking / Speed fields already use, so switching model
  // costs no extra request. `null` = not authoritative (offline runtime, still
  // loading, or discovery failed) and must not trigger any clearing.
  const modelsQuery = useQuery(
    runtimeModelsOptions(
      canDiscoverRuntimeModels ? agent.runtime_id : null,
      runtime?.workspace_id,
    ),
  );
  const modelCatalog = useMemo<ModelCatalog>(
    () =>
      modelsQuery.isSuccess
        ? modelsQuery.data.supported
          ? modelsQuery.data.models
          : []
        : null,
    [modelsQuery.data, modelsQuery.isSuccess],
  );
  const handleModelChange = useCallback(
    (model: string) =>
      update(
        buildModelChangeUpdate({
          provider: runtime?.provider ?? "",
          model,
          thinkingLevel: agent.thinking_level ?? "",
          serviceTier: agent.service_tier ?? "",
          catalog: modelCatalog,
        }),
      ),
    [
      agent.service_tier,
      agent.thinking_level,
      modelCatalog,
      runtime?.provider,
      update,
    ],
  );

  return (
    <div className="space-y-8">
      <SettingsSection
        title={t(($) => $.inspector.section_execution)}
        description={t(($) => $.inspector.section_execution_hint)}
      >
        <SettingsCard>
          <SettingsRow label={t(($) => $.inspector.prop_model)} size="none">
            <ModelPicker
              variant="chip"
              showLabel={false}
              runtimeId={agent.runtime_id}
              runtimeOnline={canDiscoverRuntimeModels}
              value={agent.model ?? ""}
              canEdit={canEdit}
              provider={runtime?.provider}
              thinkingLevel={agent.thinking_level ?? ""}
              serviceTier={agent.service_tier ?? ""}
              runtimes={runtimes
                .filter(
                  (item) =>
                    item.id === agent.runtime_id ||
                    isRuntimeUsableForUser(item, currentUserId),
                )
                .map((item) => ({
                  ...item,
                  selectable:
                    item.id !== agent.runtime_id ||
                    isRuntimeUsableForUser(item, currentUserId),
                }))}
              onSelection={(selection) => {
                const sameRuntime = selection.runtimeId === agent.runtime_id;
                const selectedEntry = sameRuntime
                  ? findModelCapabilityEntry(
                      selection.catalog ?? [],
                      selection.model,
                      runtime?.provider ?? "",
                    )
                  : undefined;
                const modelChange = sameRuntime
                  ? buildModelChangeUpdate({
                      provider: runtime?.provider ?? "",
                      model: selection.model,
                      thinkingLevel: agent.thinking_level ?? "",
                      serviceTier: agent.service_tier ?? "",
                      catalog: selection.catalog,
                    })
                  : null;
                const keepExistingThinking =
                  sameRuntime &&
                  Boolean(selection.model) &&
                  !selection.thinkingLevel &&
                  !selectedEntry &&
                  modelChange?.thinking_level !== "";
                update({
                  ...(sameRuntime ? {} : { runtime_id: selection.runtimeId }),
                  model: selection.model,
                  thinking_level: keepExistingThinking
                    ? (agent.thinking_level ?? "")
                    : selection.thinkingLevel,
                  service_tier: selection.serviceTier,
                });
              }}
              onChange={handleModelChange}
            />
          </SettingsRow>
          <SettingsRow
            label={t(($) => $.inspector.prop_concurrency)}
            size="select-wide"
          >
            <ConcurrencyField
              value={agent.max_concurrent_tasks}
              canEdit={canEdit}
              onSave={(next) => update({ max_concurrent_tasks: next })}
            />
          </SettingsRow>
        </SettingsCard>
      </SettingsSection>
    </div>
  );
}

function ConcurrencyField({
  value,
  canEdit,
  onSave,
}: {
  value: number;
  canEdit: boolean;
  onSave: (next: number) => Promise<void>;
}) {
  const { t } = useT("agents");
  const [draft, setDraft] = useState(String(value));

  useEffect(() => setDraft(String(value)), [value]);

  const commit = () => {
    const next = Number(draft);
    if (
      !Number.isInteger(next) ||
      next < AGENT_MAX_CONCURRENT_TASKS_MIN ||
      next > AGENT_MAX_CONCURRENT_TASKS_MAX
    ) {
      setDraft(String(value));
      return;
    }
    if (next !== value) void onSave(next);
  };

  return (
    <div>
      <Input
        id="agent-concurrency"
        type="number"
        name="agent-concurrency"
        autoComplete="off"
        inputMode="numeric"
        min={AGENT_MAX_CONCURRENT_TASKS_MIN}
        max={AGENT_MAX_CONCURRENT_TASKS_MAX}
        value={draft}
        onChange={(event) => setDraft(event.target.value)}
        onBlur={commit}
        onKeyDown={(event) => {
          if (isImeComposing(event)) return;
          if (event.key === "Enter") {
            event.preventDefault();
            commit();
          }
        }}
        disabled={!canEdit}
        aria-label={t(($) => $.inspector.prop_concurrency)}
        className="font-mono tabular-nums"
      />
      <p className="mt-1 text-caption text-muted-foreground">
        {t(($) => $.pickers.concurrency_range, {
          min: AGENT_MAX_CONCURRENT_TASKS_MIN,
          max: AGENT_MAX_CONCURRENT_TASKS_MAX,
        })}
      </p>
    </div>
  );
}
