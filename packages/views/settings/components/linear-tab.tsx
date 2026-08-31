"use client";

import { useEffect, useMemo, useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { toast } from "sonner";
import { CheckCircle2, CircleAlert, Loader2, Settings2, Trash2 } from "lucide-react";
import { ApiError, api } from "@patchbay/core/api";
import {
  linearBindingsOptions,
  linearCatalogOptions,
  linearConnectionOptions,
  linearMemberBindingsOptions,
  linearKeys,
} from "@patchbay/core/linear";
import { projectListOptions } from "@patchbay/core/projects";
import { memberListOptions } from "@patchbay/core/workspace/queries";
import type {
  LinearCatalogResponse,
  LinearDryRunResponse,
  LinearMemberBinding,
  LinearProjectBinding,
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
};

const emptyDraft: BindingDraft = {
  patchbayProjectId: "",
  linearProjectId: "",
  linearTeamId: "",
  syncMode: "import",
  initialSourceOfTruth: "linear",
  statusMapping: {},
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
  const qc = useQueryClient();

  const selectedBinding = bindings.find(
    (binding) => binding.patchbay_project_id === draft.patchbayProjectId,
  );
  const selectedPatchbayProject = projects.find((project) => project.id === draft.patchbayProjectId);
  const selectedLinearProject = catalog.projects.find(
    (project) => project.id === draft.linearProjectId,
  );

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
      }));
      return;
    }
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
        await api.enqueueLinearInitialImport(workspaceId, savedBinding.id);
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
        {step < 6 ? (
          <Button onClick={goNext}>{t(($) => $.page.linear.next)}</Button>
        ) : (
          <Button
            disabled={
              saving ||
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
  const connectionQuery = useQuery(linearConnectionOptions(workspaceId));
  const bindingsQuery = useQuery(linearBindingsOptions(workspaceId));
  const membersQuery = useQuery(memberListOptions(workspaceId));
  const catalogQuery = useQuery(
    linearCatalogOptions(workspaceId, wizardOpen && Boolean(connectionQuery.data?.connection)),
  );
  const projectsQuery = useQuery({
    ...projectListOptions(workspaceId),
    enabled: wizardOpen,
  });
  const connection = connectionQuery.data?.connection;
  const isConnected = connection?.status === "active";
  const memberBindingsQuery = useQuery({
    ...linearMemberBindingsOptions(workspaceId),
    enabled: Boolean(isConnected),
  });
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
        action={action}
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
                  ? t(($) => $.page.linear.last_webhook_received, { time: lastSync })
                  : t(($) => $.page.linear.last_webhook_never)}
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
