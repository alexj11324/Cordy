"use client";

import { useEffect, useMemo, useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { toast } from "sonner";
import { CheckCircle2, CircleAlert, GitMerge, Loader2, Settings2, Trash2 } from "lucide-react";
import { ApiError, api } from "@patchbay/core/api";
import { useFeatureEnabled } from "@patchbay/core/config";
import { LINEAR_AGENT_BRIDGE_FLAG } from "@patchbay/core/feature-flags";
import {
  linearBindingsOptions,
  linearCatalogOptions,
  linearConflictsOptions,
  linearConnectionOptions,
  linearMemberBindingsOptions,
  linearKeys,
} from "@patchbay/core/linear";
import { projectListOptions } from "@patchbay/core/projects";
import { agentListOptions, memberListOptions } from "@patchbay/core/workspace/queries";
import type {
  Agent,
  LinearCatalogResponse,
  LinearDryRunResponse,
  LinearMemberBinding,
  LinearProjectBinding,
  LinearSyncConflict,
  LinearSyncMode,
  MemberWithUser,
  SaveLinearProjectBindingRequest,
} from "@patchbay/core/types";
import { Badge } from "@patchbay/ui/components/ui/badge";
import { Button } from "@patchbay/ui/components/ui/button";
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@patchbay/ui/components/ui/alert-dialog";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@patchbay/ui/components/ui/dialog";
import { useT } from "../../i18n";
import { IntegrationCard } from "./integration-card";

type LinearIntegrationCardProps = {
  canManage: boolean;
  isGuest: boolean;
  workspaceId: string;
};

type WizardStep = 1 | 2 | 3 | 4 | 5 | 6;

type BindingDraft = {
  patchbayProjectId: string;
  linearProjectId: string;
  linearTeamId: string;
  syncMode: LinearSyncMode;
  initialSourceOfTruth: "linear" | "patchbay" | null;
  statusMapping: Record<string, unknown>;
  agentLabelMapping: Record<string, unknown>;
};

const emptyDraft: BindingDraft = {
  patchbayProjectId: "",
  linearProjectId: "",
  linearTeamId: "",
  syncMode: "import",
  initialSourceOfTruth: "linear",
  statusMapping: {},
  agentLabelMapping: {},
};

function selectClassName() {
  return "h-9 w-full rounded-md border border-input bg-background px-3 text-body";
}

function connectionErrorIsConfiguration(error: unknown) {
  return error instanceof ApiError && error.status === 503;
}

function formatLastSync(value: string | null | undefined) {
  if (!value) return null;
  const timestamp = Date.parse(value);
  return Number.isNaN(timestamp) ? null : new Date(timestamp).toLocaleString();
}

function connectionLabel(status: string | undefined, t: ReturnType<typeof useT<"settings">>["t"]) {
  if (status === "active") {
    return t(($) => $.page.linear.healthy);
  }
  if (status === "reauthorization_required") {
    return t(($) => $.page.linear.reauthorization_required);
  }
  if (status === "revoked") {
    return t(($) => $.page.linear.disconnected);
  }
  return t(($) => $.page.linear.unavailable);
}

function SyncModeLabel({ mode }: { mode: LinearSyncMode }) {
  const { t } = useT("settings");
  switch (mode) {
    case "publish":
      return <>{t(($) => $.page.linear.mode_publish)}</>;
    case "two_way":
      return <>{t(($) => $.page.linear.mode_two_way)}</>;
    case "not_synced":
      return <>{t(($) => $.page.linear.mode_not_synced)}</>;
    case "import":
    default:
      return <>{t(($) => $.page.linear.mode_import)}</>;
  }
}

function formatConflictValue(value: unknown) {
  if (value === null || value === undefined) return "—";
  if (typeof value === "string") return value;
  try {
    return JSON.stringify(value);
  } catch {
    return String(value);
  }
}

function ConflictCenter({
  canManage,
  conflicts,
  onClose,
  workspaceId,
}: {
  canManage: boolean;
  conflicts: LinearSyncConflict[];
  onClose: () => void;
  workspaceId: string;
}) {
  const { t } = useT("settings");
  const qc = useQueryClient();
  const [manualValues, setManualValues] = useState<Record<string, string>>({});
  const [pendingId, setPendingId] = useState<string | null>(null);

  async function resolve(
    conflict: LinearSyncConflict,
    resolution: "local" | "remote" | "manual",
  ) {
    setPendingId(conflict.id);
    try {
      await api.resolveLinearSyncConflict(workspaceId, conflict.id, {
        resolution,
        ...(resolution === "manual"
          ? { manual_value: manualValues[conflict.id] ?? "" }
          : {}),
      });
      await qc.invalidateQueries({ queryKey: linearKeys.conflicts(workspaceId) });
      toast.success(t(($) => $.page.linear.conflict_resolved));
    } catch {
      toast.error(t(($) => $.page.linear.conflict_resolve_failed));
    } finally {
      setPendingId(null);
    }
  }

  function fieldLabel(field: string) {
    switch (field) {
      case "title":
        return t(($) => $.page.linear.conflict_field_title);
      case "description":
        return t(($) => $.page.linear.conflict_field_description);
      case "priority":
        return t(($) => $.page.linear.conflict_field_priority);
      case "status":
        return t(($) => $.page.linear.conflict_field_status);
      case "due_date":
        return t(($) => $.page.linear.conflict_field_due_date);
      case "owner_id":
        return t(($) => $.page.linear.conflict_field_owner);
      default:
        return field;
    }
  }

  function statusLabel(status: string) {
    switch (status) {
      case "open":
        return t(($) => $.page.linear.conflict_status_open);
      case "resolved":
        return t(($) => $.page.linear.conflict_status_resolved);
      case "dismissed":
        return t(($) => $.page.linear.conflict_status_dismissed);
      default:
        return status;
    }
  }

  return (
    <DialogContent className="max-h-[90vh] overflow-y-auto sm:max-w-3xl">
      <DialogHeader>
        <DialogTitle>{t(($) => $.page.linear.conflict_center)}</DialogTitle>
        <DialogDescription>{t(($) => $.page.linear.conflict_center_description)}</DialogDescription>
      </DialogHeader>
      {conflicts.length === 0 ? (
        <p className="text-body text-muted-foreground">{t(($) => $.page.linear.no_conflicts)}</p>
      ) : (
        <div className="space-y-3">
          {conflicts.map((conflict) => (
            <div className="space-y-3 rounded-lg border p-3" key={conflict.id}>
              <div className="flex items-center justify-between gap-2">
                <span className="font-medium">{fieldLabel(conflict.field)}</span>
                <Badge variant="destructive">{statusLabel(conflict.status)}</Badge>
              </div>
              <div className="grid gap-2 text-micro sm:grid-cols-3">
                <div>
                  <div className="text-muted-foreground">{t(($) => $.page.linear.conflict_base)}</div>
                  <div className="break-words">{formatConflictValue(conflict.base_value)}</div>
                </div>
                <div>
                  <div className="text-muted-foreground">{t(($) => $.page.linear.conflict_local)}</div>
                  <div className="break-words">{formatConflictValue(conflict.local_value)}</div>
                </div>
                <div>
                  <div className="text-muted-foreground">{t(($) => $.page.linear.conflict_remote)}</div>
                  <div className="break-words">{formatConflictValue(conflict.remote_value)}</div>
                </div>
              </div>
              {canManage ? (
                <div className="flex flex-wrap items-center gap-2">
                  <Button
                    disabled={pendingId === conflict.id}
                    onClick={() => void resolve(conflict, "local")}
                    size="sm"
                    variant="outline"
                  >
                    {t(($) => $.page.linear.conflict_use_local)}
                  </Button>
                  <Button
                    disabled={pendingId === conflict.id}
                    onClick={() => void resolve(conflict, "remote")}
                    size="sm"
                    variant="outline"
                  >
                    {t(($) => $.page.linear.conflict_use_remote)}
                  </Button>
                  <input
                    aria-label={t(($) => $.page.linear.conflict_manual)}
                    className="h-9 min-w-48 flex-1 rounded-md border border-input bg-background px-3 text-body"
                    onChange={(event) =>
                      setManualValues((current) => ({
                        ...current,
                        [conflict.id]: event.target.value,
                      }))
                    }
                    placeholder={t(($) => $.page.linear.conflict_manual_placeholder)}
                    value={manualValues[conflict.id] ?? ""}
                  />
                  <Button
                    disabled={pendingId === conflict.id || !(manualValues[conflict.id] ?? "").trim()}
                    onClick={() => void resolve(conflict, "manual")}
                    size="sm"
                    variant="outline"
                  >
                    {t(($) => $.page.linear.conflict_use_manual)}
                  </Button>
                </div>
              ) : (
                <p className="text-micro text-muted-foreground">
                  {t(($) => $.page.linear.conflict_read_only)}
                </p>
              )}
            </div>
          ))}
        </div>
      )}
      <DialogFooter>
        <Button onClick={onClose} variant="outline">{t(($) => $.page.linear.conflict_close)}</Button>
      </DialogFooter>
    </DialogContent>
  );
}

function WizardProgress({ step }: { step: WizardStep }) {
  const { t } = useT("settings");
  const labels = [
    t(($) => $.page.linear.step_connect),
    t(($) => $.page.linear.step_match),
    t(($) => $.page.linear.step_mode),
    t(($) => $.page.linear.step_mapping),
    t(($) => $.page.linear.step_preview),
    t(($) => $.page.linear.step_activate),
  ];
  return (
    <ol className="grid grid-cols-3 gap-2 text-micro sm:grid-cols-6">
      {labels.map((label, index) => {
        const number = index + 1;
        return (
          <li
            className={number === step ? "font-semibold text-foreground" : "text-muted-foreground"}
            key={label}
          >
            <span
              className={
                number <= step
                  ? "mr-1 inline-flex size-5 items-center justify-center rounded-full bg-primary text-primary-foreground"
                  : "mr-1 inline-flex size-5 items-center justify-center rounded-full bg-muted"
              }
            >
              {number}
            </span>
            {label}
          </li>
        );
      })}
    </ol>
  );
}

function BindingWizard({
  bindings,
  catalog,
  connectionId,
  agents,
  agentBridgeEnabled,
  memberBindings,
  members,
  pullImportEnabled,
  onClose,
  onSaved,
  projects,
  workspaceId,
}: {
  bindings: LinearProjectBinding[];
  catalog: LinearCatalogResponse;
  connectionId: string;
  agents: Agent[];
  agentBridgeEnabled: boolean;
  memberBindings: LinearMemberBinding[];
  members: MemberWithUser[];
  pullImportEnabled: boolean;
  onClose: () => void;
  onSaved: () => void;
  projects: readonly { id: string; title: string }[];
  workspaceId: string;
}) {
  const { t } = useT("settings");
  const [step, setStep] = useState<WizardStep>(1);
  const [draft, setDraft] = useState<BindingDraft>(emptyDraft);
  const [saving, setSaving] = useState(false);
  const [dryRun, setDryRun] = useState<LinearDryRunResponse | null>(null);
  const [dryRunLoading, setDryRunLoading] = useState(false);
  const [dryRunError, setDryRunError] = useState(false);
  const [memberMappings, setMemberMappings] = useState<Record<string, string>>(() =>
    Object.fromEntries(
      memberBindings.map((binding) => [binding.patchbay_user_id, binding.linear_user_id]),
    ),
  );
  const [importRetryBindingId, setImportRetryBindingId] = useState<string | null>(null);
  const [retryingImport, setRetryingImport] = useState(false);
  const qc = useQueryClient();

  const selectedBinding = bindings.find(
    (binding) => binding.patchbay_project_id === draft.patchbayProjectId,
  );
  const selectedPatchbayProject = projects.find((project) => project.id === draft.patchbayProjectId);
  const selectedLinearProject = catalog.projects.find(
    (project) => project.id === draft.linearProjectId,
  );
  const agentLabelGroups = catalog.labels.filter(
    (label) =>
      label.is_group && (label.team_id === null || label.team_id === draft.linearTeamId),
  );
  const selectedAgentLabelGroupId =
    typeof draft.agentLabelMapping.group_id === "string"
      ? draft.agentLabelMapping.group_id
      : "";
  const selectedAgentLabelChildren = catalog.labels.filter(
    (label) => !label.is_group && label.parent_id === selectedAgentLabelGroupId,
  );
  const activeAgents = agents.filter((agent) => !agent.archived_at);
  const agentLabelAssignments =
    typeof draft.agentLabelMapping.labels === "object" &&
    draft.agentLabelMapping.labels !== null &&
    !Array.isArray(draft.agentLabelMapping.labels)
      ? (draft.agentLabelMapping.labels as Record<string, unknown>)
      : {};

  const suggestedLinearProject = useMemo(() => {
    const title = selectedPatchbayProject?.title.trim().toLocaleLowerCase();
    if (!title) return undefined;
    return catalog.projects.find((project) => project.name.trim().toLocaleLowerCase() === title);
  }, [catalog.projects, selectedPatchbayProject?.title]);

  useEffect(() => {
    if (!draft.patchbayProjectId && projects[0]) {
      setDraft((current) => ({ ...current, patchbayProjectId: projects[0]?.id ?? "" }));
    }
  }, [draft.patchbayProjectId, projects]);

  useEffect(() => {
    if (!draft.patchbayProjectId) return;
    const existing = bindings.find(
      (binding) => binding.patchbay_project_id === draft.patchbayProjectId,
    );
    if (existing) {
      setDraft((current) => ({
        ...current,
        linearProjectId: existing.linear_project_id,
        linearTeamId: existing.linear_team_id ?? "",
        syncMode: existing.sync_mode,
        initialSourceOfTruth: existing.initial_source_of_truth === "patchbay" ? "patchbay" : "linear",
        statusMapping: existing.status_mapping,
        agentLabelMapping: existing.agent_label_mapping,
      }));
      return;
    }
    setDraft((current) => ({ ...current, agentLabelMapping: {} }));
    if (suggestedLinearProject && !draft.linearProjectId) {
      setDraft((current) => ({
        ...current,
        linearProjectId: suggestedLinearProject.id,
      }));
    }
  }, [bindings, draft.patchbayProjectId, draft.linearProjectId, suggestedLinearProject]);

  useEffect(() => {
    if (
      step !== 5 ||
      !connectionId ||
      !draft.patchbayProjectId ||
      !draft.linearProjectId
    ) {
      setDryRun(null);
      setDryRunLoading(false);
      setDryRunError(false);
      return;
    }
    let cancelled = false;
    setDryRun(null);
    setDryRunError(false);
    setDryRunLoading(true);
    const body: SaveLinearProjectBindingRequest = {
      connection_id: connectionId,
      patchbay_project_id: draft.patchbayProjectId,
      linear_project_id: draft.linearProjectId,
      linear_team_id: draft.linearTeamId || null,
      status: draft.syncMode === "not_synced" ? "draft" : "active",
      sync_mode: draft.syncMode,
      initial_source_of_truth:
        draft.syncMode === "not_synced" ? null : draft.initialSourceOfTruth,
      status_mapping: draft.statusMapping,
      agent_label_mapping: draft.agentLabelMapping,
    };
    void api
      .dryRunLinearBinding(workspaceId, body)
      .then((result) => {
        if (!cancelled) setDryRun(result);
      })
      .catch(() => {
        if (!cancelled) setDryRunError(true);
      })
      .finally(() => {
        if (!cancelled) setDryRunLoading(false);
      });
    return () => {
      cancelled = true;
    };
  }, [
    connectionId,
    draft.initialSourceOfTruth,
    draft.linearProjectId,
    draft.linearTeamId,
    draft.patchbayProjectId,
    draft.statusMapping,
    draft.agentLabelMapping,
    draft.syncMode,
    step,
    workspaceId,
  ]);

  function setDraftValue<T extends keyof BindingDraft>(key: T, value: BindingDraft[T]) {
    setDraft((current) => ({ ...current, [key]: value }));
  }

  function goNext() {
    if (step === 2 && (!draft.patchbayProjectId || !draft.linearProjectId)) {
      toast.error(t(($) => $.page.linear.match_required));
      return;
    }
    if (step === 3 && draft.syncMode !== "not_synced" && !draft.linearTeamId) {
      toast.error(t(($) => $.page.linear.team_required));
      setStep(2);
      return;
    }
    setStep((current) => (current < 6 ? (current + 1) as WizardStep : current));
  }

  function goBack() {
    setStep((current) => (current > 1 ? (current - 1) as WizardStep : current));
  }

  async function saveBinding() {
    if (!draft.patchbayProjectId || !draft.linearProjectId) {
      toast.error(t(($) => $.page.linear.match_required));
      setStep(2);
      return;
    }
    setSaving(true);
    const body: SaveLinearProjectBindingRequest = {
      connection_id: "",
      patchbay_project_id: draft.patchbayProjectId,
      linear_project_id: draft.linearProjectId,
      linear_team_id: draft.linearTeamId || null,
      status: draft.syncMode === "not_synced" ? "draft" : "active",
      sync_mode: draft.syncMode,
      initial_source_of_truth: draft.syncMode === "not_synced" ? null : draft.initialSourceOfTruth,
      status_mapping: draft.statusMapping,
      agent_label_mapping: draft.agentLabelMapping,
    };
    try {
      const connection = await api.getLinearConnection(workspaceId);
      if (!connection.connection) {
        throw new Error("Linear connection disappeared");
      }
      body.connection_id = connection.connection.id;
      const savedBinding = selectedBinding
        ? await api.updateLinearBinding(workspaceId, selectedBinding.id, body)
        : await api.createLinearBinding(workspaceId, body);
      const shouldQueueInitialImport =
        pullImportEnabled &&
        (draft.syncMode === "import" ||
          (draft.syncMode === "two_way" && draft.initialSourceOfTruth === "linear")) &&
        (!selectedBinding ||
          selectedBinding.status !== "active" ||
          selectedBinding.sync_mode !== draft.syncMode ||
          selectedBinding.initial_source_of_truth !== draft.initialSourceOfTruth);
      if (shouldQueueInitialImport && savedBinding.status === "active") {
        try {
          await api.enqueueLinearInitialImport(workspaceId, savedBinding.id);
        } catch {
          setImportRetryBindingId(savedBinding.id);
          await qc.invalidateQueries({ queryKey: linearKeys.bindings(workspaceId) });
          toast.error(t(($) => $.page.linear.import_queue_failed));
          return;
        }
      }
      await Promise.all(
        members.map(async (member) => {
          const linearUserId = memberMappings[member.user_id]?.trim() ?? "";
          const existing = memberBindings.some(
            (binding) => binding.patchbay_user_id === member.user_id,
          );
          if (linearUserId) {
            await api.saveLinearMemberBinding(workspaceId, {
              connection_id: body.connection_id,
              patchbay_user_id: member.user_id,
              linear_user_id: linearUserId,
            });
          } else if (existing) {
            await api.deleteLinearMemberBinding(workspaceId, member.user_id);
          }
        }),
      );
      setImportRetryBindingId(null);
      await qc.invalidateQueries({ queryKey: linearKeys.bindings(workspaceId) });
      await qc.invalidateQueries({ queryKey: linearKeys.memberBindings(workspaceId) });
      toast.success(t(($) => $.page.linear.saved));
      onSaved();
      onClose();
    } catch (error) {
      toast.error(
        error instanceof ApiError && error.status === 409
          ? t(($) => $.page.linear.save_conflict)
          : t(($) => $.page.linear.save_failed),
      );
    } finally {
      setSaving(false);
    }
  }

  async function retryInitialImport() {
    if (!importRetryBindingId) return;
    setRetryingImport(true);
    try {
      await api.enqueueLinearInitialImport(workspaceId, importRetryBindingId);
      setImportRetryBindingId(null);
      await qc.invalidateQueries({ queryKey: linearKeys.bindings(workspaceId) });
      toast.success(t(($) => $.page.linear.saved));
      onSaved();
      onClose();
    } catch {
      toast.error(t(($) => $.page.linear.import_queue_failed));
    } finally {
      setRetryingImport(false);
    }
  }

  return (
    <DialogContent className="max-h-[90vh] overflow-y-auto sm:max-w-3xl">
      <DialogHeader>
        <DialogTitle>{t(($) => $.page.linear.wizard_title)}</DialogTitle>
        <DialogDescription>{t(($) => $.page.linear.wizard_description)}</DialogDescription>
      </DialogHeader>
      <WizardProgress step={step} />

      {step === 1 ? (
        <div className="space-y-4 rounded-lg border p-4">
          <h4 className="font-medium">{t(($) => $.page.linear.connect_title)}</h4>
          <p className="text-body text-muted-foreground">
            {t(($) => $.page.linear.connected_identity)}
          </p>
        </div>
      ) : null}

      {step === 2 ? (
        <div className="space-y-4 rounded-lg border p-4">
          <h4 className="font-medium">{t(($) => $.page.linear.match_title)}</h4>
          <div className="grid gap-3 sm:grid-cols-2">
            <label className="space-y-1.5 text-body">
              <span className="text-muted-foreground">{t(($) => $.page.linear.patchbay_project)}</span>
              <select
                className={selectClassName()}
                value={draft.patchbayProjectId}
                onChange={(event) =>
                  setDraft((current) => ({
                    ...current,
                    patchbayProjectId: event.target.value,
                    linearProjectId: "",
                    linearTeamId: "",
                    syncMode: "import",
                    initialSourceOfTruth: "linear",
                    statusMapping: {},
                  }))
                }
              >
                <option value="">{t(($) => $.page.linear.select_project)}</option>
                {projects.map((project) => (
                  <option key={project.id} value={project.id}>
                    {project.title}
                  </option>
                ))}
              </select>
            </label>
            <label className="space-y-1.5 text-body">
              <span className="text-muted-foreground">{t(($) => $.page.linear.linear_project)}</span>
              <select
                className={selectClassName()}
                value={draft.linearProjectId}
                onChange={(event) => setDraftValue("linearProjectId", event.target.value)}
              >
                <option value="">{t(($) => $.page.linear.select_project)}</option>
                {catalog.projects.map((project) => (
                  <option key={project.id} value={project.id}>
                    {project.name}
                  </option>
                ))}
              </select>
            </label>
            <label className="space-y-1.5 text-body sm:col-span-2">
              <span className="text-muted-foreground">{t(($) => $.page.linear.linear_team)}</span>
              <select
                className={selectClassName()}
                value={draft.linearTeamId}
                onChange={(event) => setDraftValue("linearTeamId", event.target.value)}
              >
                <option value="">{t(($) => $.page.linear.select_team)}</option>
                {catalog.teams.map((team) => (
                  <option key={team.id} value={team.id}>
                    {team.key} · {team.name}
                  </option>
                ))}
              </select>
            </label>
          </div>
          {suggestedLinearProject && suggestedLinearProject.id === draft.linearProjectId ? (
            <p className="text-micro text-muted-foreground">
              {t(($) => $.page.linear.name_match_suggestion)}
            </p>
          ) : null}
        </div>
      ) : null}

      {step === 3 ? (
        <div className="space-y-3 rounded-lg border p-4">
          <h4 className="font-medium">{t(($) => $.page.linear.mode_title)}</h4>
          {(["import", "publish", "two_way", "not_synced"] as const).map((mode) => (
            <label className="flex cursor-pointer items-start gap-3 rounded-md border p-3" key={mode}>
              <input
                checked={draft.syncMode === mode}
                name="linear-sync-mode"
                onChange={() =>
                  setDraft((current) => ({
                    ...current,
                    syncMode: mode,
                    initialSourceOfTruth:
                      mode === "import" ? "linear" : mode === "publish" ? "patchbay" : mode === "not_synced" ? null : current.initialSourceOfTruth ?? "linear",
                  }))
                }
                type="radio"
              />
              <span className="space-y-0.5 text-body">
                <span className="block font-medium"><SyncModeLabel mode={mode} /></span>
                <span className="block text-muted-foreground">
                  {mode === "two_way"
                    ? t(($) => $.page.linear.mode_two_way_hint)
                    : mode === "publish"
                      ? t(($) => $.page.linear.mode_publish_hint)
                      : mode === "not_synced"
                        ? t(($) => $.page.linear.mode_not_synced_hint)
                        : t(($) => $.page.linear.mode_import_hint)}
                </span>
              </span>
            </label>
          ))}
          {draft.syncMode === "two_way" ? (
            <label className="space-y-1.5 text-body">
              <span className="text-muted-foreground">{t(($) => $.page.linear.initial_source)}</span>
              <select
                className={selectClassName()}
                value={draft.initialSourceOfTruth ?? "linear"}
                onChange={(event) =>
                  setDraftValue("initialSourceOfTruth", event.target.value as "linear" | "patchbay")
                }
              >
                <option value="linear">{t(($) => $.page.linear.source_linear)}</option>
                <option value="patchbay">{t(($) => $.page.linear.source_patchbay)}</option>
              </select>
            </label>
          ) : null}
        </div>
      ) : null}

      {step === 4 ? (
        <div className="space-y-4 rounded-lg border p-4">
          <h4 className="font-medium">{t(($) => $.page.linear.mapping_title)}</h4>
          <p className="text-body text-muted-foreground">{t(($) => $.page.linear.mapping_description)}</p>
          {catalog.states.length === 0 ? (
            <p className="text-body text-muted-foreground">{t(($) => $.page.linear.no_states)}</p>
          ) : (
            <div className="space-y-2">
              {catalog.states.map((state) => (
                <label className="grid items-center gap-2 text-body sm:grid-cols-[1fr_1fr]" key={state.id}>
                  <span>{state.name}</span>
                  <select
                    className={selectClassName()}
                    value={String(draft.statusMapping[state.id] ?? "")}
                    onChange={(event) =>
                      setDraft((current) => ({
                        ...current,
                        statusMapping: {
                          ...current.statusMapping,
                          [state.id]: event.target.value,
                        },
                      }))
                    }
                  >
                    <option value="">{t(($) => $.page.linear.leave_unmapped)}</option>
                    <option value="backlog">{t(($) => $.page.linear.status_backlog)}</option>
                    <option value="todo">{t(($) => $.page.linear.status_todo)}</option>
                    <option value="in_progress">{t(($) => $.page.linear.status_in_progress)}</option>
                    <option value="in_review">{t(($) => $.page.linear.status_in_review)}</option>
                    <option value="done">{t(($) => $.page.linear.status_done)}</option>
                    <option value="blocked">{t(($) => $.page.linear.status_blocked)}</option>
                    <option value="cancelled">{t(($) => $.page.linear.status_cancelled)}</option>
                  </select>
                </label>
              ))}
            </div>
          )}
          {agentBridgeEnabled ? (
            <div className="space-y-3 border-t pt-4">
              <h4 className="font-medium">{t(($) => $.page.linear.agent_mapping_title)}</h4>
              <p className="text-body text-muted-foreground">
                {t(($) => $.page.linear.agent_mapping_description)}
              </p>
              <label className="space-y-1.5 text-body">
                <span className="text-muted-foreground">
                  {t(($) => $.page.linear.agent_label_group)}
                </span>
                <select
                  className={selectClassName()}
                  value={selectedAgentLabelGroupId}
                  onChange={(event) =>
                    setDraft((current) =>
                      event.target.value
                        ? {
                            ...current,
                            agentLabelMapping: {
                              group_id: event.target.value,
                              labels: {},
                            },
                          }
                        : { ...current, agentLabelMapping: {} },
                    )
                  }
                >
                  <option value="">{t(($) => $.page.linear.select_agent_label_group)}</option>
                  {agentLabelGroups.map((group) => (
                    <option key={group.id} value={group.id}>
                      {group.name}
                    </option>
                  ))}
                </select>
              </label>
              {selectedAgentLabelGroupId && selectedAgentLabelChildren.length === 0 ? (
                <p className="text-body text-muted-foreground">
                  {t(($) => $.page.linear.no_agent_label_values)}
                </p>
              ) : null}
              {selectedAgentLabelChildren.map((label) => (
                <label
                  className="grid items-center gap-2 text-body sm:grid-cols-[1fr_1fr]"
                  key={label.id}
                >
                  <span>{label.name}</span>
                  <select
                    className={selectClassName()}
                    value={String(agentLabelAssignments[label.id] ?? "")}
                    onChange={(event) =>
                      setDraft((current) => {
                        const currentLabels =
                          typeof current.agentLabelMapping.labels === "object" &&
                          current.agentLabelMapping.labels !== null &&
                          !Array.isArray(current.agentLabelMapping.labels)
                            ? { ...(current.agentLabelMapping.labels as Record<string, unknown>) }
                            : {};
                        if (event.target.value) {
                          currentLabels[label.id] = event.target.value;
                        } else {
                          delete currentLabels[label.id];
                        }
                        return {
                          ...current,
                          agentLabelMapping: {
                            ...current.agentLabelMapping,
                            labels: currentLabels,
                          },
                        };
                      })
                    }
                  >
                    <option value="">{t(($) => $.page.linear.agent_not_mapped)}</option>
                    {activeAgents.map((agent) => (
                      <option key={agent.id} value={agent.id}>
                        {agent.name}
                      </option>
                    ))}
                  </select>
                </label>
              ))}
              {selectedAgentLabelGroupId ? (
                <label className="space-y-1.5 text-body">
                  <span className="text-muted-foreground">
                    {t(($) => $.page.linear.agent_default)}
                  </span>
                  <select
                    className={selectClassName()}
                    value={String(draft.agentLabelMapping.default_agent_id ?? "")}
                    onChange={(event) =>
                      setDraft((current) => {
                        const mapping = { ...current.agentLabelMapping };
                        if (event.target.value) {
                          mapping.default_agent_id = event.target.value;
                        } else {
                          delete mapping.default_agent_id;
                        }
                        return { ...current, agentLabelMapping: mapping };
                      })
                    }
                  >
                    <option value="">{t(($) => $.page.linear.agent_no_default)}</option>
                    {activeAgents.map((agent) => (
                      <option key={agent.id} value={agent.id}>
                        {agent.name}
                      </option>
                    ))}
                  </select>
                </label>
              ) : null}
            </div>
          ) : null}
          <div className="space-y-2 border-t pt-4">
            <h4 className="font-medium">{t(($) => $.page.linear.member_mapping_title)}</h4>
            <p className="text-body text-muted-foreground">
              {t(($) => $.page.linear.member_mapping_description)}
            </p>
            {members.length === 0 ? (
              <p className="text-body text-muted-foreground">
                {t(($) => $.page.linear.no_members)}
              </p>
            ) : (
              members.map((member) => (
                <label
                  className="grid items-center gap-2 text-body sm:grid-cols-[1fr_1fr]"
                  key={member.user_id}
                >
                  <span>{member.name || member.email}</span>
                  <select
                    className={selectClassName()}
                    value={memberMappings[member.user_id] ?? ""}
                    onChange={(event) =>
                      setMemberMappings((current) => ({
                        ...current,
                        [member.user_id]: event.target.value,
                      }))
                    }
                  >
                    <option value="">{t(($) => $.page.linear.member_not_mapped)}</option>
                    {catalog.users.map((user) => (
                      <option key={user.id} value={user.id}>
                        {user.name}{user.email ? ` · ${user.email}` : ""}
                      </option>
                    ))}
                  </select>
                </label>
              ))
            )}
          </div>
        </div>
      ) : null}

      {step === 5 ? (
        <div className="space-y-4 rounded-lg border p-4">
          <h4 className="font-medium">{t(($) => $.page.linear.preview_title)}</h4>
          <div className="grid gap-3 sm:grid-cols-3">
            <div className="rounded-md bg-muted/50 p-3">
              <div className="text-micro text-muted-foreground">{t(($) => $.page.linear.preview_project)}</div>
              <div className="mt-1 text-body font-medium">{selectedPatchbayProject?.title ?? "—"}</div>
            </div>
            <div className="rounded-md bg-muted/50 p-3">
              <div className="text-micro text-muted-foreground">{t(($) => $.page.linear.preview_linear_project)}</div>
              <div className="mt-1 text-body font-medium">{selectedLinearProject?.name ?? "—"}</div>
            </div>
            <div className="rounded-md bg-muted/50 p-3">
              <div className="text-micro text-muted-foreground">{t(($) => $.page.linear.preview_mode)}</div>
              <div className="mt-1 text-body font-medium"><SyncModeLabel mode={draft.syncMode} /></div>
            </div>
          </div>
          {dryRunLoading ? (
            <p className="flex items-center gap-2 text-body text-muted-foreground">
              <Loader2 className="animate-spin" />
              {t(($) => $.page.linear.preview_loading)}
            </p>
          ) : dryRunError ? (
            <p className="text-body text-destructive">{t(($) => $.page.linear.preview_failed)}</p>
          ) : dryRun ? (
            <>
              <div className="grid gap-3 sm:grid-cols-2 lg:grid-cols-4">
                <div className="rounded-md bg-muted/50 p-3">
                  <div className="text-micro text-muted-foreground">{t(($) => $.page.linear.preview_local_issues)}</div>
                  <div className="mt-1 text-body font-medium">{dryRun.local_issue_count}</div>
                </div>
                <div className="rounded-md bg-muted/50 p-3">
                  <div className="text-micro text-muted-foreground">{t(($) => $.page.linear.preview_remote_issues)}</div>
                  <div className="mt-1 text-body font-medium">{dryRun.remote_issue_count}</div>
                </div>
                <div className="rounded-md bg-muted/50 p-3">
                  <div className="text-micro text-muted-foreground">{t(($) => $.page.linear.preview_candidate_import)}</div>
                  <div className="mt-1 text-body font-medium">{dryRun.candidate_import_count}</div>
                </div>
                <div className="rounded-md bg-muted/50 p-3">
                  <div className="text-micro text-muted-foreground">{t(($) => $.page.linear.preview_candidate_publish)}</div>
                  <div className="mt-1 text-body font-medium">{dryRun.candidate_publish_count}</div>
                </div>
              </div>
              {dryRun.unmapped_remote_status_count > 0 ? (
                <p className="text-body text-destructive">
                  {t(($) => $.page.linear.preview_unmapped_statuses, {
                    count: dryRun.unmapped_remote_status_count,
                  })}
                </p>
              ) : null}
              {dryRun.remote_issue_count_truncated ? (
                <p className="text-body text-destructive">{t(($) => $.page.linear.preview_truncated)}</p>
              ) : null}
              <p className="text-body text-muted-foreground">{t(($) => $.page.linear.preview_estimated)}</p>
              <p className="text-body text-muted-foreground">{t(($) => $.page.linear.preview_no_mutations)}</p>
            </>
          ) : null}
        </div>
      ) : null}

      {step === 6 ? (
        <div className="space-y-4 rounded-lg border p-4">
          <h4 className="font-medium">{t(($) => $.page.linear.activate_title)}</h4>
          <p className="text-body text-muted-foreground">
            {t(($) => $.page.linear.activate_description)}
          </p>
          <dl className="grid gap-2 text-body sm:grid-cols-2">
            <div><dt className="text-muted-foreground">{t(($) => $.page.linear.linear_project)}</dt><dd>{selectedLinearProject?.name ?? "—"}</dd></div>
            <div><dt className="text-muted-foreground">{t(($) => $.page.linear.linear_team)}</dt><dd>{catalog.teams.find((team) => team.id === draft.linearTeamId)?.name ?? "—"}</dd></div>
            <div><dt className="text-muted-foreground">{t(($) => $.page.linear.mode_title)}</dt><dd><SyncModeLabel mode={draft.syncMode} /></dd></div>
            <div><dt className="text-muted-foreground">{t(($) => $.page.linear.status_mapping_count)}</dt><dd>{Object.keys(draft.statusMapping).filter((key) => draft.statusMapping[key]).length}</dd></div>
          </dl>
        </div>
      ) : null}

      <DialogFooter className="gap-2 sm:justify-between">
        <Button disabled={step === 1 || saving} onClick={goBack} variant="ghost">
          {t(($) => $.page.linear.back)}
        </Button>
        {importRetryBindingId ? (
          <Button
            disabled={retryingImport}
            onClick={() => void retryInitialImport()}
            variant="outline"
          >
            {retryingImport ? <Loader2 className="animate-spin" /> : null}
            {t(($) => $.page.linear.retry_import)}
          </Button>
        ) : null}
        {step < 6 ? (
          <Button onClick={goNext}>{t(($) => $.page.linear.next)}</Button>
        ) : (
          <Button
            disabled={
              saving ||
              importRetryBindingId !== null ||
              dryRunLoading ||
              !dryRun ||
              dryRunError ||
              dryRun.remote_issue_count_truncated
            }
            onClick={() => void saveBinding()}
          >
            {saving ? <Loader2 className="animate-spin" /> : null}
            {t(($) => $.page.linear.activate)}
          </Button>
        )}
      </DialogFooter>
    </DialogContent>
  );
}

export function LinearIntegrationCard({
  canManage,
  isGuest,
  workspaceId,
}: LinearIntegrationCardProps) {
  const { t } = useT("settings");
  const qc = useQueryClient();
  const [wizardOpen, setWizardOpen] = useState(false);
  const [disconnectOpen, setDisconnectOpen] = useState(false);
  const [conflictOpen, setConflictOpen] = useState(false);
  const agentBridgeFlagEnabled = useFeatureEnabled(LINEAR_AGENT_BRIDGE_FLAG, false);
  const connectionQuery = useQuery(linearConnectionOptions(workspaceId));
  const agentBridgeEnabled =
    agentBridgeFlagEnabled && (connectionQuery.data?.agent_bridge_enabled ?? false);
  const bindingsQuery = useQuery(linearBindingsOptions(workspaceId));
  const membersQuery = useQuery(memberListOptions(workspaceId));
  const catalogQuery = useQuery(
    linearCatalogOptions(workspaceId, wizardOpen && Boolean(connectionQuery.data?.connection)),
  );
  const projectsQuery = useQuery({
    ...projectListOptions(workspaceId),
    enabled: wizardOpen,
  });
  const agentsQuery = useQuery({
    ...agentListOptions(workspaceId),
    enabled: wizardOpen && agentBridgeEnabled,
  });
  const connection = connectionQuery.data?.connection;
  const isConnected = connection?.status === "active";
  const memberBindingsQuery = useQuery({
    ...linearMemberBindingsOptions(workspaceId),
    enabled: Boolean(isConnected),
  });
  const conflictsQuery = useQuery({
    ...linearConflictsOptions(workspaceId),
    enabled: Boolean(isConnected),
  });
  const openConflicts = conflictsQuery.data?.conflicts ?? [];
  const isReauthorizationRequired = connection?.status === "reauthorization_required";
  const hasUnknownStatus = Boolean(
    connection && !["active", "reauthorization_required", "revoked"].includes(connection.status),
  );
  const configured = connectionQuery.data?.configured ?? false;
  const bindings = bindingsQuery.data?.bindings ?? [];
  const activeBindingCount = bindings.filter(
    (binding) => binding.status === "active" && binding.sync_mode !== "not_synced",
  ).length;
  const lastSync = formatLastSync(connection?.last_success_at);

  async function connect() {
    try {
      const response = await api.connectLinear(workspaceId);
      if (!response.authorization_url) {
        throw new Error("Linear authorization URL was empty");
      }
      window.location.assign(response.authorization_url);
    } catch {
      toast.error(t(($) => $.page.linear.connect_failed));
    }
  }

  async function disconnect() {
    try {
      await api.disconnectLinear(workspaceId);
      await qc.invalidateQueries({ queryKey: linearKeys.all(workspaceId) });
      toast.success(t(($) => $.page.linear.disconnected_toast));
      setDisconnectOpen(false);
    } catch {
      toast.error(t(($) => $.page.linear.disconnect_failed));
    }
  }

  const status = connectionQuery.isLoading ? (
    <Badge variant="secondary"><Loader2 className="animate-spin" />{t(($) => $.page.linear.loading)}</Badge>
  ) : connectionQuery.isError ? (
    <Badge variant="outline"><CircleAlert />{connectionErrorIsConfiguration(connectionQuery.error) ? t(($) => $.page.linear.not_configured) : t(($) => $.page.linear.unavailable)}</Badge>
  ) : !configured ? (
    <Badge variant="outline">{t(($) => $.page.linear.not_configured)}</Badge>
  ) : isReauthorizationRequired ? (
    <Badge variant="destructive"><CircleAlert />{t(($) => $.page.linear.reauthorization_required)}</Badge>
  ) : hasUnknownStatus ? (
    <Badge variant="outline"><CircleAlert />{connectionLabel(connection?.status, t)}</Badge>
  ) : isConnected ? (
    <Badge className="bg-emerald-600 text-white hover:bg-emerald-600"><CheckCircle2 />{connectionLabel(connection?.status, t)}</Badge>
  ) : (
    <Badge variant="outline">{t(($) => $.page.linear.disconnected)}</Badge>
  );

  let action;
  if (isGuest) {
    action = <span className="text-caption text-muted-foreground">{t(($) => $.page.linear.login_required)}</span>;
  } else if (!canManage) {
    action = <span className="text-caption text-muted-foreground">{t(($) => $.page.linear.admin_only)}</span>;
  } else if (connectionQuery.isLoading) {
    action = <Loader2 className="size-4 animate-spin text-muted-foreground" />;
  } else if (!configured) {
    action = <span className="text-caption text-muted-foreground">{t(($) => $.page.linear.server_configuration_required)}</span>;
  } else if (isReauthorizationRequired || !isConnected) {
    action = <Button onClick={() => void connect()} size="sm"><Settings2 />{isReauthorizationRequired ? t(($) => $.page.linear.reconnect) : t(($) => $.page.linear.connect)}</Button>;
  } else {
    action = (
      <div className="flex flex-wrap items-center justify-end gap-2">
        <Button onClick={() => setWizardOpen(true)} size="sm" variant="outline"><Settings2 />{t(($) => $.page.linear.manage)}</Button>
        <Button onClick={() => void connect()} size="sm" variant="outline">{t(($) => $.page.linear.reconnect)}</Button>
        <Button className="text-destructive hover:text-destructive" onClick={() => setDisconnectOpen(true)} size="sm" variant="ghost"><Trash2 />{t(($) => $.page.linear.disconnect)}</Button>
      </div>
    );
  }

  return (
    <>
      <IntegrationCard
        action={
          <div className="flex flex-wrap items-center justify-end gap-2">
            {openConflicts.length > 0 ? (
              <Button onClick={() => setConflictOpen(true)} size="sm" variant="outline">
                <GitMerge />{t(($) => $.page.linear.conflict_center)}
              </Button>
            ) : null}
            {action}
          </div>
        }
        channel="linear"
        description={t(($) => $.page.linear.description)}
        iconClassName="bg-[#5E6AD2]/10"
        status={
          <div className="flex flex-wrap items-center gap-2">
            {status}
            {isConnected ? (
              <span className="text-micro text-muted-foreground">
                {connection?.organization_name} · {t(($) => $.page.linear.projects_synced, { count: activeBindingCount })}
                {" · "}
                {lastSync
                  ? t(($) => $.page.linear.last_sync, { time: lastSync })
                  : t(($) => $.page.linear.last_sync_never)}
                {openConflicts.length > 0
                  ? ` · ${t(($) => $.page.linear.conflicts_count, { count: openConflicts.length })}`
                  : ""}
              </span>
            ) : null}
          </div>
        }
        title={t(($) => $.page.linear.title)}
      />
      <Dialog open={wizardOpen} onOpenChange={setWizardOpen}>
        {wizardOpen && catalogQuery.data ? (
          <BindingWizard
            bindings={bindings}
            catalog={catalogQuery.data}
            connectionId={connection?.id ?? ""}
            agents={agentsQuery.data ?? []}
            agentBridgeEnabled={agentBridgeEnabled}
            memberBindings={memberBindingsQuery.data?.bindings ?? []}
            members={membersQuery.data ?? []}
            pullImportEnabled={connectionQuery.data?.pull_import_enabled ?? false}
            onClose={() => setWizardOpen(false)}
            onSaved={() => void qc.invalidateQueries({ queryKey: linearKeys.connection(workspaceId) })}
            projects={projectsQuery.data ?? []}
            workspaceId={workspaceId}
          />
        ) : (
          <DialogContent>
            <DialogHeader>
              <DialogTitle>{t(($) => $.page.linear.loading_catalog)}</DialogTitle>
              <DialogDescription>
                {catalogQuery.isError ? t(($) => $.page.linear.catalog_failed) : t(($) => $.page.linear.loading_catalog_description)}
              </DialogDescription>
            </DialogHeader>
            {catalogQuery.isLoading ? <Loader2 className="mx-auto animate-spin" /> : null}
          </DialogContent>
        )}
      </Dialog>
      <Dialog open={conflictOpen} onOpenChange={setConflictOpen}>
        {conflictOpen ? (
          <ConflictCenter
            canManage={canManage}
            conflicts={openConflicts}
            onClose={() => setConflictOpen(false)}
            workspaceId={workspaceId}
          />
        ) : null}
      </Dialog>
      <AlertDialog open={disconnectOpen} onOpenChange={setDisconnectOpen}>
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>{t(($) => $.page.linear.disconnect_title)}</AlertDialogTitle>
            <AlertDialogDescription>{t(($) => $.page.linear.disconnect_description)}</AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>{t(($) => $.page.linear.cancel)}</AlertDialogCancel>
            <AlertDialogAction onClick={() => void disconnect()}>{t(($) => $.page.linear.disconnect)}</AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </>
  );
}
