"use client";

import { useCallback, useEffect, useId, useMemo, useState } from "react";
import { LogOut } from "lucide-react";
import { Button, Input } from "@lobehub/ui/base-ui";
import { toast } from "sonner";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useAuthStore } from "@orvilo/core/auth";
import { useDeleteWorkspace } from "@orvilo/core/workspace/mutations";
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
import { SettingsSaveState, type SettingsSaveStatus } from "./settings-layout";
import { SettingsFormRow } from "./settings-shell";
import { useSettingsConfirm } from "./settings-confirm";
import { useAutoSave } from "./use-auto-save";
import {
  Frame,
  FrameHeader,
  FramePanel,
  FrameTitle,
} from "@orvilo/ui/components/reui/frame";
import { Separator } from "@orvilo/ui/components/ui/separator";

/**
 * Workspace — logo, name, URL and issue prefix, plus the owner-only danger
 * zone.
 *
 * The `name` field is a `useAutoSave` draft and the issue prefix is a
 * hand-rolled variant of the same idea (it confirms before it writes), so
 * neither carries a `name` and the shell's rows are layout only. The logo and
 * the URL are not drafts at all: the logo uploads and PATCHes on its own, and
 * the URL is derived from the current route.
 *
 * `Frame`/`FramePanel` stay: this tab's card and its save-state header bar are
 * ReUI chrome the migration does not replace (decision 11). What changed inside
 * is the row layer — `Field`/`FieldLabel`/`FieldContent`/`Separator` became the
 * shell's `SettingsFormRow`, and the inputs and buttons are Lobe's.
 *
 * `htmlFor` on the fields that have a control to label; the two enum-free rows
 * below (the avatar) have none to point at.
 */

interface WorkspaceDetailsDraft {
  name: string;
}

function workspaceDetailsEqual(
  left: WorkspaceDetailsDraft,
  right: WorkspaceDetailsDraft,
) {
  return left.name === right.name;
}

/**
 * The old `SettingsRow`'s `text` tier, in pixels — every text field in the card
 * shares it so their edges line up.
 */
const TEXT_MIN_WIDTH = 384;

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
  const confirm = useSettingsConfirm();
  // The two cards' accessible names. `Role="group"` on the panel plus
  // `aria-labelledby` on its own title is the same pairing `SettingsGroup`
  // builds for the Frame-less tabs — see that component for why the label is
  // an id reference rather than a repeated string.
  const generalLabelId = useId();
  const dangerLabelId = useId();

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
      // Let the rejection out: `useSettingsConfirm` keeps the dialog open only
      // while its `onOk` promise is unsettled, and a prefix the server refused
      // is exactly the case where dismissing would claim a rename that never
      // happened. The status readout in the card header shows the failure once
      // the user cancels.
      throw error;
    }
  };

  const handlePrefixBlur = () => {
    if (!workspace || prefixInvalid || !prefixChanged) return;
    const nextPrefix = normalizedPrefix;
    confirm({
      title: t(($) => $.workspace.prefix_confirm_title),
      description: t(($) => $.workspace.prefix_confirm_description, {
        oldPrefix: workspace.issue_prefix,
        newPrefix: nextPrefix,
      }),
      confirmLabel: t(($) => $.workspace.confirm_action),
      cancelLabel: t(($) => $.workspace.confirm_cancel),
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
    <div className="flex flex-col gap-8">
      <Frame variant="ghost" spacing="sm" className="bg-transparent p-0">
        <FramePanel
          aria-labelledby={generalLabelId}
          className="rounded-lg bg-muted/30 p-0 shadow-none"
          role="group"
        >
          <FrameHeader className="flex-row items-center justify-between gap-3 px-4 py-3">
            {/* The card had no heading before, and the rows inside it had no
                group to belong to: `frame.tsx` renders `FrameTitle` as a plain
                `<div>`, so a screen reader met a run of unnamed labels and the
                save-state region with nothing tying them together. The copy is
                the `section_general` string the old atom layer left unused. */}
            <FrameTitle id={generalLabelId}>
              {t(($) => $.workspace.section_general)}
            </FrameTitle>
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
          <div className="px-4">
            <SettingsFormRow
              disabled={!canManageWorkspace}
              label={t(($) => $.workspace.logo_label)}
            >
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
            </SettingsFormRow>

            <SettingsFormRow
              disabled={!canManageWorkspace}
              divider
              label={t(($) => $.workspace.name_label)}
              htmlFor="workspace-name"
              minWidth={TEXT_MIN_WIDTH}
            >
              <Input
                className="w-full"
                autoComplete="organization"
                disabled={!canManageWorkspace}
                id="workspace-name"
                name="workspace-name"
                type="text"
                value={name}
                onBlur={detailsAutoSave.flush}
                onChange={(event) => setName(event.target.value)}
              />
            </SettingsFormRow>

            <SettingsFormRow
              divider
              label={t(($) => $.workspace.url_label)}
              htmlFor="workspace-url"
              minWidth={TEXT_MIN_WIDTH}
            >
              <Input
                className="w-full"
                id="workspace-url"
                name="workspace-url"
                readOnly
                type="url"
                value={workspaceUrl}
              />
            </SettingsFormRow>

            <SettingsFormRow
              disabled={!canManageWorkspace}
              divider
              label={t(($) => $.workspace.issue_prefix_label)}
              htmlFor="workspace-issue-prefix"
              minWidth={TEXT_MIN_WIDTH}
            >
              <Input
                className="w-full"
                aria-invalid={prefixInvalid}
                autoCapitalize="characters"
                autoComplete="off"
                disabled={!canManageWorkspace}
                id="workspace-issue-prefix"
                maxLength={10}
                name="workspace-issue-prefix"
                placeholder={workspace.issue_prefix}
                spellCheck={false}
                type="text"
                value={issuePrefix}
                onBlur={handlePrefixBlur}
                onChange={(event) => {
                  setPrefixSaveStatus("idle");
                  setIssuePrefix(normalizePrefix(event.target.value));
                }}
              />
            </SettingsFormRow>
          </div>
        </FramePanel>
      </Frame>

      {/* Gate the owner-only Delete button on the member query settling. */}
      {membersFetched && isOwner && (
        <Frame variant="ghost" spacing="sm" className="bg-transparent p-0">
          <FramePanel
            aria-labelledby={dangerLabelId}
            className="rounded-lg bg-muted/30 p-0 shadow-none"
            role="group"
          >
            <FrameHeader className="px-4 py-3">
              <FrameTitle className="flex items-center gap-2" id={dangerLabelId}>
                <LogOut
                  aria-hidden="true"
                  className="size-4 text-muted-foreground"
                />
                {t(($) => $.workspace.danger_zone)}
              </FrameTitle>
            </FrameHeader>
            <Separator />
            <div className="px-4">
              <SettingsFormRow
                label={
                  <span className="text-destructive">
                    {t(($) => $.workspace.delete_title)}
                  </span>
                }
              >
                <Button
                  danger
                  disabled={actionId === "delete-workspace"}
                  id="workspace-delete"
                  shape="round"
                  size="small"
                  type="primary"
                  onClick={() => setDeleteDialogOpen(true)}
                >
                  {actionId === "delete-workspace"
                    ? t(($) => $.workspace.deleting)
                    : t(($) => $.workspace.delete_button)}
                </Button>
              </SettingsFormRow>
            </div>
          </FramePanel>
        </Frame>
      )}

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
    </div>
  );
}
