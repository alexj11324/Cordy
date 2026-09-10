"use client";

import { useCallback, useEffect, useMemo, useState } from "react";
import { LogOut } from "lucide-react";
import {
  AlertDialog,
  AlertDialogContent,
  AlertDialogHeader,
  AlertDialogTitle,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogCancel,
  AlertDialogAction,
} from "@orvilo/ui/components/ui/alert-dialog";
import { toast } from "sonner";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useAuthStore } from "@orvilo/core/auth";
import {
  useDeleteWorkspace,
} from "@orvilo/core/workspace/mutations";
import {
  memberListOptions,
  workspaceKeys,
  workspaceListOptions,
} from "@orvilo/core/workspace/queries";
import { issueKeys } from "@orvilo/core/issues/queries";
import { api } from "@orvilo/core/api";
import {
  paths,
  resolvePostAuthDestination,
  useCurrentWorkspace,
  useHasOnboarded,
} from "@orvilo/core/paths";
import { setCurrentWorkspace } from "@orvilo/core/platform";
import type { Workspace } from "@orvilo/core/types";
import { AvatarUploadControl } from "../../common/avatar-upload-control";
import { useNavigation } from "../../navigation";
import { DeleteWorkspaceDialog } from "./delete-workspace-dialog";
import { useT } from "../../i18n";
import {
  SettingsSaveState,
  SettingsTab,
  type SettingsSaveStatus,
} from "./settings-layout";
import { useAutoSave } from "./use-auto-save";
import {
  Frame,
  FrameHeader,
  FramePanel,
  FrameTitle,
} from "@orvilo/ui/components/reui/frame";
import {
  Field,
  FieldContent,
  FieldGroup,
  FieldLabel,
} from "@orvilo/ui/components/ui/field";
import { Input } from "@orvilo/ui/components/ui/input";
import { Separator } from "@orvilo/ui/components/ui/separator";
import { Button } from "@orvilo/ui/components/ui/button";

interface WorkspaceDetailsDraft {
  name: string;
}

function workspaceDetailsEqual(
  left: WorkspaceDetailsDraft,
  right: WorkspaceDetailsDraft,
) {
  return left.name === right.name;
}

export function WorkspaceTab() {
  const { t } = useT("settings");
  const user = useAuthStore((s) => s.user);
  const workspace = useCurrentWorkspace();
  // Derive the id from useCurrentWorkspace instead of the throwing
  // useWorkspaceId: this component can legitimately render while the
  // workspace is gone from the list cache but the URL slug hasn't changed
  // yet (post-delete invalidation before navigation completes, or an
  // external delete of the workspace we're on). The `!workspace` guard
  // below renders null for that window; a throwing hook would crash first.
  const wsId = workspace?.id;
  const { data: members = [], isFetched: membersFetched } = useQuery({
    ...memberListOptions(wsId ?? ""),
    enabled: !!wsId,
  });
  const qc = useQueryClient();
  const deleteWorkspace = useDeleteWorkspace();
  const navigation = useNavigation();
  const hasOnboarded = useHasOnboarded();

  /**
   * Send the user to a safe URL, computed from the current cached workspace
   * list minus the workspace that's going away.
   *
   * Delete calls this AFTER the mutation succeeds. The realtime
   * `workspace:deleted` handler skips self-initiated deletes (see
   * `pending-delete.ts`), so nothing races this navigation.
   */
  const navigateAwayFromCurrentWorkspace = () => {
    const cachedList =
      qc.getQueryData<Workspace[]>(workspaceListOptions().queryKey) ?? [];
    const remaining = cachedList.filter((w) => w.id !== workspace?.id);
    // Clear the workspace-context singleton BEFORE navigating. Three
    // downstream consumers read it:
    //  1. Realtime relocate handlers' "current === lost workspace" check —
    //     if the singleton still points at the lost workspace when the WS
    //     event arrives, they fire a parallel full-page relocate that races
    //     this navigation.
    //  2. Chrome gating (`{slug && <AppSidebar />}` on desktop) — if the
    //     singleton lingers, the sidebar stays mounted while the deleted
    //     workspace is no longer in the list, and `useWorkspaceId` throws.
    //  3. API client's `X-Workspace-Slug` header — stale header post-
    //     delete is at best a 404, at worst leaks into the next query.
    // WorkspaceRouteLayout re-sets the singleton when a new workspace's
    // route mounts; clearing here is safe — either the next workspace
    // takes over immediately, or the new-workspace overlay takes over
    // (which has no workspace context, so null is correct).
    setCurrentWorkspace(null, null);
    navigation.push(resolvePostAuthDestination(remaining, hasOnboarded));
  };

  const [name, setName] = useState(workspace?.name ?? "");
  const [issuePrefix, setIssuePrefix] = useState(workspace?.issue_prefix ?? "");
  const [prefixSaveStatus, setPrefixSaveStatus] =
    useState<SettingsSaveStatus>("idle");
  const [actionId, setActionId] = useState<string | null>(null);
  const [confirmAction, setConfirmAction] = useState<{
    title: string;
    description: string;
    variant?: "destructive";
    onConfirm: () => Promise<void>;
  } | null>(null);
  const [deleteDialogOpen, setDeleteDialogOpen] = useState(false);

  const currentMember = members.find((m) => m.user_id === user?.id) ?? null;
  const canManageWorkspace =
    currentMember?.role === "owner" || currentMember?.role === "admin";
  const isOwner = currentMember?.role === "owner";

  // Reset form state only when the user switches to a different workspace.
  // Keying on workspace?.id (not the object ref) avoids wiping unsaved edits
  // when an unrelated mutation — e.g. avatar/logo upload — replaces the
  // cached Workspace object via setQueryData.
  useEffect(() => {
    setName(workspace?.name ?? "");
    setIssuePrefix(workspace?.issue_prefix ?? "");
    // eslint-disable-next-line react-hooks/exhaustive-deps -- intentionally keyed on id only; see comment above
  }, [workspace?.id]);

  // Letters + digits only, uppercase, capped at 10 chars. The backend
  // uppercases and trims on its side too — this is purely a UX guardrail
  // so the value the user sees in the input matches what gets persisted.
  const normalizePrefix = (raw: string) =>
    raw
      .toUpperCase()
      .replace(/[^A-Z0-9]/g, "")
      .slice(0, 10);

  const normalizedPrefix = normalizePrefix(issuePrefix);
  const prefixChanged =
    !!workspace && normalizedPrefix !== workspace.issue_prefix;
  const prefixInvalid = normalizedPrefix.length === 0;

  const detailsDraft = useMemo(
    () => ({ name }),
    [name],
  );
  const savedDetails = useMemo(
    () => ({ name: workspace?.name ?? "" }),
    [workspace?.name],
  );
  const saveDetails = useCallback(
    async (next: WorkspaceDetailsDraft) => {
      if (!workspace) return;
      const updated = await api.updateWorkspace(workspace.id, {
        name: next.name,
      });
      qc.setQueryData(workspaceKeys.list(), (old: Workspace[] | undefined) =>
        old?.map((ws) => (ws.id === updated.id ? updated : ws)),
      );
    },
    [qc, workspace],
  );
  const detailsAutoSave = useAutoSave({
    value: detailsDraft,
    savedValue: savedDetails,
    onSave: saveDetails,
    onSuccess: () =>
      toast.success(
        t(($) => $.workspace.toast_saved),
        {
          id: "settings-auto-save",
        },
      ),
    onError: (error) =>
      toast.error(
        error instanceof Error
          ? error.message
          : t(($) => $.workspace.toast_save_failed),
      ),
    enabled: !!workspace && canManageWorkspace && !!name.trim(),
    isEqual: workspaceDetailsEqual,
  });

  const performPrefixSave = async (nextPrefix: string) => {
    if (!workspace) return;
    setPrefixSaveStatus("saving");
    try {
      const updated = await api.updateWorkspace(workspace.id, {
        issue_prefix: nextPrefix,
      });
      qc.setQueryData(workspaceKeys.list(), (old: Workspace[] | undefined) =>
        old?.map((ws) => (ws.id === updated.id ? updated : ws)),
      );
      // Issue identifiers are computed from the workspace prefix at read time,
      // so every cached issue key is stale after this confirmed change.
      await qc.invalidateQueries({ queryKey: issueKeys.all(updated.id) });
      setPrefixSaveStatus("saved");
      toast.success(
        t(($) => $.workspace.toast_saved),
        {
          id: "settings-auto-save",
        },
      );
    } catch (error) {
      setPrefixSaveStatus("error");
      toast.error(
        error instanceof Error
          ? error.message
          : t(($) => $.workspace.toast_save_failed),
      );
    }
  };

  const handlePrefixBlur = () => {
    if (!workspace || prefixInvalid || !prefixChanged) return;
    const nextPrefix = normalizedPrefix;
    setConfirmAction({
      title: t(($) => $.workspace.prefix_confirm_title),
      description: t(($) => $.workspace.prefix_confirm_description, {
        oldPrefix: workspace.issue_prefix,
        newPrefix: nextPrefix,
      }),
      variant: "destructive",
      onConfirm: () => performPrefixSave(nextPrefix),
    });
  };

  const handleConfirmDelete = async () => {
    if (!workspace) return;
    setActionId("delete-workspace");
    // Await the DELETE with the dialog in its loading state, and only
    // navigate on success (CLAUDE.md: flows that navigate must await the
    // server; no optimistic removal). The realtime `workspace:deleted`
    // handler skips self-initiated deletes via the pending-delete registry,
    // so it can't race this navigation with its own full-page relocate.
    // On failure the dialog stays open, the cache was never touched, and
    // the user is exactly where they started.
    try {
      await deleteWorkspace.mutateAsync(workspace.id);
      setDeleteDialogOpen(false);
      navigateAwayFromCurrentWorkspace();
    } catch (e) {
      toast.error(
        e instanceof Error
          ? e.message
          : t(($) => $.workspace.toast_delete_failed),
      );
    } finally {
      setActionId(null);
    }
  };

  if (!workspace) return null;

  const workspaceUrl = navigation.getShareableUrl(
    paths.workspace(workspace.slug).issues(),
  );

  return (
    <SettingsTab
      title={t(($) => $.page.tabs.general)}
    >
      <div className="flex flex-col gap-8">
        <Frame variant="ghost" spacing="sm" className="bg-transparent p-0">
          <FramePanel className="rounded-lg bg-muted/30 p-0 shadow-none">
            <FrameHeader className="flex-row justify-end px-4 py-3">
              <SettingsSaveState
                status={
                  prefixSaveStatus === "saving" || prefixSaveStatus === "error"
                    ? prefixSaveStatus
                    : detailsAutoSave.status === "idle"
                      ? prefixSaveStatus
                      : detailsAutoSave.status
                }
                savingLabel={t(($) => $.auto_save.saving)}
                savedLabel={t(($) => $.auto_save.saved)}
                errorLabel={t(($) => $.auto_save.failed)}
              />
            </FrameHeader>
            <Separator />
            <FieldGroup className="gap-0 px-4 py-1">
              <Field
                orientation="responsive"
                data-disabled={!canManageWorkspace || undefined}
                className="py-3 @md/field-group:gap-6"
              >
                <FieldContent className="@md/field-group:w-32 @md/field-group:flex-none">
                  <FieldLabel>{t(($) => $.workspace.logo_label)}</FieldLabel>
                </FieldContent>
                <FieldContent className="min-w-0 @md/field-group:flex-1">
                  <div className="flex justify-start @md/field-group:justify-end">
                    <AvatarUploadControl
                      variant="workspace"
                      value={workspace.avatar_url ?? null}
                      name={workspace.name}
                      size={64}
                      disabled={!canManageWorkspace}
                      ariaLabel={t(($) => $.workspace.change_logo_aria)}
                      onUploaded={async (url) => {
                        try {
                          const updated = await api.updateWorkspace(
                            workspace.id,
                            {
                              avatar_url: url,
                            },
                          );
                          qc.setQueryData(
                            workspaceKeys.list(),
                            (old: Workspace[] | undefined) =>
                              old?.map((ws) =>
                                ws.id === updated.id ? updated : ws,
                              ),
                          );
                          toast.success(
                            t(($) => $.workspace.toast_logo_updated),
                            {
                              id: "settings-auto-save",
                            },
                          );
                        } catch (error) {
                          toast.error(
                            error instanceof Error
                              ? error.message
                              : t(($) => $.workspace.toast_logo_failed),
                          );
                        }
                      }}
                    />
                  </div>
                </FieldContent>
              </Field>
              <Separator />

              <Field
                orientation="responsive"
                data-disabled={!canManageWorkspace || undefined}
                className="py-3 @md/field-group:gap-6"
              >
                <FieldContent className="@md/field-group:w-32 @md/field-group:flex-none">
                  <FieldLabel htmlFor="workspace-name">
                    {t(($) => $.workspace.name_label)}
                  </FieldLabel>
                </FieldContent>
                <FieldContent className="min-w-0 @md/field-group:flex-1">
                  <Input
                    id="workspace-name"
                    type="text"
                    name="workspace-name"
                    autoComplete="organization"
                    value={name}
                    onChange={(event) => setName(event.target.value)}
                    onBlur={detailsAutoSave.flush}
                    disabled={!canManageWorkspace}
                  />
                </FieldContent>
              </Field>
              <Separator />

              <Field
                orientation="responsive"
                className="py-3 @md/field-group:gap-6"
              >
                <FieldContent className="@md/field-group:w-32 @md/field-group:flex-none">
                  <FieldLabel htmlFor="workspace-url">
                    {t(($) => $.workspace.url_label)}
                  </FieldLabel>
                </FieldContent>
                <FieldContent className="min-w-0 @md/field-group:flex-1">
                  <Input
                    id="workspace-url"
                    type="url"
                    name="workspace-url"
                    value={workspaceUrl}
                    readOnly
                  />
                </FieldContent>
              </Field>
              <Separator />

              <Field
                orientation="responsive"
                data-disabled={!canManageWorkspace || undefined}
                className="py-3 @md/field-group:gap-6"
              >
                <FieldContent className="@md/field-group:w-32 @md/field-group:flex-none">
                  <FieldLabel htmlFor="workspace-issue-prefix">
                    {t(($) => $.workspace.issue_prefix_label)}
                  </FieldLabel>
                </FieldContent>
                <FieldContent className="min-w-0 @md/field-group:flex-1">
                  <Input
                    id="workspace-issue-prefix"
                    type="text"
                    name="workspace-issue-prefix"
                    autoComplete="off"
                    autoCapitalize="characters"
                    spellCheck={false}
                    value={issuePrefix}
                    onChange={(event) => {
                      setPrefixSaveStatus("idle");
                      setIssuePrefix(normalizePrefix(event.target.value));
                    }}
                    onBlur={handlePrefixBlur}
                    disabled={!canManageWorkspace}
                    maxLength={10}
                    aria-invalid={prefixInvalid}
                    placeholder={workspace.issue_prefix}
                  />
                </FieldContent>
              </Field>
            </FieldGroup>
          </FramePanel>
        </Frame>

        {/* Gate the owner-only Delete button on the member query settling. */}
        {membersFetched && isOwner && (
          <Frame variant="ghost" spacing="sm" className="bg-transparent p-0">
            <FramePanel className="rounded-lg bg-muted/30 p-0 shadow-none">
              <FrameHeader className="px-4 py-3">
                <FrameTitle className="flex items-center gap-2">
                  <LogOut
                    aria-hidden="true"
                    className="size-4 text-muted-foreground"
                  />
                  {t(($) => $.workspace.danger_zone)}
                </FrameTitle>
              </FrameHeader>
              <Separator />
              <FieldGroup className="gap-0 px-4 py-1">
                <Field
                  orientation="responsive"
                  data-disabled={actionId === "delete-workspace" || undefined}
                  className="py-3 @md/field-group:gap-6"
                >
                  <FieldContent className="@md/field-group:w-32 @md/field-group:flex-none">
                    <FieldLabel
                      htmlFor="workspace-delete"
                      className="text-destructive"
                    >
                      {t(($) => $.workspace.delete_title)}
                    </FieldLabel>
                  </FieldContent>
                  <FieldContent className="min-w-0 @md/field-group:flex-1 @md/field-group:items-start">
                    <Button
                      id="workspace-delete"
                      type="button"
                      variant="destructive"
                      size="sm"
                      onClick={() => setDeleteDialogOpen(true)}
                      disabled={actionId === "delete-workspace"}
                    >
                      {actionId === "delete-workspace"
                        ? t(($) => $.workspace.deleting)
                        : t(($) => $.workspace.delete_button)}
                    </Button>
                  </FieldContent>
                </Field>
              </FieldGroup>
            </FramePanel>
          </Frame>
        )}
      </div>

      <AlertDialog
        open={!!confirmAction}
        onOpenChange={(v) => {
          if (!v) setConfirmAction(null);
        }}
      >
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>{confirmAction?.title}</AlertDialogTitle>
            <AlertDialogDescription>
              {confirmAction?.description}
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>
              {t(($) => $.workspace.confirm_cancel)}
            </AlertDialogCancel>
            <AlertDialogAction
              variant={
                confirmAction?.variant === "destructive"
                  ? "destructive"
                  : "default"
              }
              onClick={async () => {
                await confirmAction?.onConfirm();
                setConfirmAction(null);
              }}
            >
              {t(($) => $.workspace.confirm_action)}
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>

      <DeleteWorkspaceDialog
        workspaceName={workspace.name}
        loading={actionId === "delete-workspace"}
        open={deleteDialogOpen}
        onOpenChange={(open) => {
          // Ignore close requests while the delete mutation is in flight
          // so the user can't accidentally dismiss mid-operation.
          if (actionId === "delete-workspace" && !open) return;
          setDeleteDialogOpen(open);
        }}
        onConfirm={handleConfirmDelete}
      />
    </SettingsTab>
  );
}
