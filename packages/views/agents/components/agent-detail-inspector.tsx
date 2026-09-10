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
import { FieldGroup } from "@orvilo/ui/components/ui/field";
import { useT } from "../../i18n";
import { ModelPicker } from "./inspector/model-picker";
import {
  buildModelChangeUpdate,
  type ModelCatalog,
} from "./inspector/model-change-cleanup";
import { findModelCapabilityEntry } from "./inspector/model-capability";
import { SettingField } from "./setting-field";

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
 * Execution settings for one agent: which model it runs and how many tasks
 * it may run in parallel.
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
    <section className="w-full space-y-3 border-b pb-6">
      <header className="space-y-1">
        <h3 className="text-body font-medium">{t(($) => $.inspector.section_execution)}</h3>
        <p className="flex flex-wrap items-center gap-2 text-caption text-muted-foreground">
          <span>
            {runtime?.name ?? t(($) => $.inspector.runtime_unassigned)}
          </span>
          <span
            aria-hidden="true"
            className="size-1 rounded-full bg-muted-foreground/50"
          />
          <span>{t(($) => $.inspector.section_execution_hint)}</span>
        </p>
      </header>

        <FieldGroup className="gap-0">
        <SettingField
          title={t(($) => $.inspector.prop_model)}
          description={t(($) => $.inspector.prop_model_hint)}
          badge={
            runtime?.provider
              ? {
                  label: runtime.provider,
                  variant: "primary-light",
                }
              : undefined
          }
          labelFor="agent-model-picker"
        >
          <div className="flex justify-start sm:justify-end">
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
          </div>
        </SettingField>

        <SettingField
          title={t(($) => $.inspector.prop_concurrency)}
          description={t(($) => $.pickers.concurrency_range, {
            min: AGENT_MAX_CONCURRENT_TASKS_MIN,
            max: AGENT_MAX_CONCURRENT_TASKS_MAX,
          })}
          badge={{
            label: t(($) => $.inspector.concurrency_slots, {
              count: agent.max_concurrent_tasks,
            }),
            variant: "info-light",
          }}
          labelFor="agent-concurrency"
          last
        >
          <div className="w-full sm:max-w-[200px] sm:ml-auto">
            <ConcurrencyField
              value={agent.max_concurrent_tasks}
              canEdit={canEdit}
              onSave={(next) => update({ max_concurrent_tasks: next })}
            />
          </div>
        </SettingField>
        </FieldGroup>
    </section>
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
    <div className="relative">
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
        className="font-mono tabular-nums text-right pr-12 h-9"
      />
      <span className="pointer-events-none absolute right-3 top-1/2 -translate-y-1/2 text-caption text-muted-foreground">
        {t(($) => $.inspector.concurrency_unit)}
      </span>
    </div>
  );
}
