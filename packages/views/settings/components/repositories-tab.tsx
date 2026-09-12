"use client";

import { useCallback, useEffect, useMemo, useState } from "react";
import { FolderOpen, FolderGit, LoaderCircle, Plus } from "lucide-react";
import { Button, Checkbox, Input, Modal } from "@lobehub/ui/base-ui";
import { Badge } from "@orvilo/ui/components/ui/badge";
import { toast } from "sonner";
import {
  useInfiniteQuery,
  useQuery,
  useQueryClient,
} from "@tanstack/react-query";
import { useAuthStore } from "@orvilo/core/auth";
import { useWorkspaceId } from "@orvilo/core/hooks";
import { useCurrentWorkspace } from "@orvilo/core/paths";
import { memberListOptions, workspaceKeys } from "@orvilo/core/workspace/queries";
import {
  githubInstallationRepositoriesOptions,
  githubInstallationsOptions,
} from "@orvilo/core/github";
import { api } from "@orvilo/core/api";
import type {
  GitHubRepository,
  Workspace,
  WorkspaceRepo,
} from "@orvilo/core/types";
import { projectListOptions } from "@orvilo/core/projects";
import { useModalStore } from "@orvilo/core/modals";
import { ProjectResourcesSection } from "../../projects/components/project-resources-section";
import { isDesktopShell } from "../../platform/local-directory";
import { githubShortLabel, repositoryIdentity } from "../../common/github-url";
export { repositoryIdentity } from "../../common/github-url";
import { useNavigation } from "../../navigation";
import { useT } from "../../i18n";
import { SettingsSaveState } from "./settings-layout";
import { useSettingsConfirm } from "./settings-confirm";
import { SettingsEmptyState } from "./settings-empty";
import { SettingsSearchBar } from "./settings-search";
import { SettingsSelect } from "./settings-select";
import { SettingsFormRow, SettingsGroup } from "./settings-shell";
import { useAutoSave } from "./use-auto-save";
import { GitHubMark } from "./github-mark";

const EMPTY_REPOSITORIES: WorkspaceRepo[] = [];

function repositoriesEqual(left: WorkspaceRepo[], right: WorkspaceRepo[]) {
  if (left.length !== right.length) return false;
  return left.every(
    (repo, index) =>
      repo.url === right[index]?.url &&
      (repo.description ?? "") === (right[index]?.description ?? ""),
  );
}

export function RepositoriesTab() {
  const { t } = useT("settings");
  const user = useAuthStore((state) => state.user);
  const workspace = useCurrentWorkspace();
  const wsId = useWorkspaceId();
  const queryClient = useQueryClient();
  const navigation = useNavigation();
  const { data: members = [] } = useQuery(memberListOptions(wsId));
  const { data: projects = [] } = useQuery(projectListOptions(wsId));
  const [remoteDialogOpen, setRemoteDialogOpen] = useState(false);
  const [remoteDraft, setRemoteDraft] = useState("");
  const [repositories, setRepositories] = useState<WorkspaceRepo[]>(
    workspace?.repos ?? EMPTY_REPOSITORIES,
  );
  // The removal `AlertDialog` is gone: the imperative confirm owns its open
  // state, so the row passes the index straight through.
  const confirm = useSettingsConfirm();
  const [connectingGitHub, setConnectingGitHub] = useState(false);
  const [githubPickerOpen, setGitHubPickerOpen] = useState(false);
  const [selectedInstallationID, setSelectedInstallationID] = useState("");
  const [selectedRepositories, setSelectedRepositories] = useState<
    Map<number, GitHubRepository>
  >(new Map());
  const [repositorySearch, setRepositorySearch] = useState("");

  const currentMember = members.find((member) => member.user_id === user?.id) ?? null;
  const canManageWorkspace =
    currentMember?.role === "owner" || currentMember?.role === "admin";
  const {
    data: githubData,
    isPending: githubInstallationsPending,
    isFetching: githubInstallationsFetching,
  } = useQuery({
    ...githubInstallationsOptions(wsId),
    enabled: !!wsId && canManageWorkspace,
  });
  const githubInstallations = useMemo(
    () => githubData?.installations ?? [],
    [githubData?.installations],
  );
  const githubConnectConfigured = githubData?.configured === true;
  const githubBrowseConfigured =
    githubData?.repository_browse_configured === true;
  const githubRepositoriesQuery = useInfiniteQuery({
    ...githubInstallationRepositoriesOptions(wsId, selectedInstallationID),
    enabled:
      githubPickerOpen &&
      canManageWorkspace &&
      githubBrowseConfigured &&
      !!selectedInstallationID,
  });
  const githubRepositories = useMemo(
    () =>
      githubRepositoriesQuery.data?.pages.flatMap(
        (page) => page.repositories,
      ) ?? [],
    [githubRepositoriesQuery.data?.pages],
  );
  const existingRepositoryIdentities = useMemo(
    () =>
      new Set(
        repositories
          .map((repository) => repositoryIdentity(repository.url))
          .filter((identity): identity is string => !!identity),
      ),
    [repositories],
  );
  const filteredGitHubRepositories = useMemo(() => {
    const search = repositorySearch.trim().toLowerCase();
    if (!search) return githubRepositories;
    return githubRepositories.filter((repository) =>
      repository.full_name.toLowerCase().includes(search),
    );
  }, [githubRepositories, repositorySearch]);

  useEffect(() => {
    setRepositories(workspace?.repos ?? EMPTY_REPOSITORIES);
    // A cache update after auto-save replaces the Workspace object. Keying on
    // identity prevents that response from wiping a newer local keystroke.
    // eslint-disable-next-line react-hooks/exhaustive-deps -- intentionally keyed on workspace identity
  }, [workspace?.id]);

  useEffect(() => {
    if (
      selectedInstallationID &&
      githubInstallations.some(
        (installation) => installation.id === selectedInstallationID,
      )
    ) {
      return;
    }
    setSelectedInstallationID(githubInstallations[0]?.id ?? "");
  }, [githubInstallations, selectedInstallationID]);

  useEffect(() => {
    const connected = navigation.searchParams.get("github_connected") === "1";
    const githubError = navigation.searchParams.get("github_error");
    if ((!connected && !githubError) || !canManageWorkspace) return;
    if (
      !githubError &&
      (githubInstallationsPending || githubInstallationsFetching)
    ) {
      return;
    }

    if (githubError) {
      toast.error(t(($) => $.repositories.github_connect_failed));
    } else if (githubInstallations.length > 0 && githubBrowseConfigured) {
      setSelectedInstallationID(githubInstallations[0]!.id);
      setGitHubPickerOpen(true);
    } else if (githubInstallations.length > 0) {
      toast.error(t(($) => $.repositories.github_browse_not_configured));
    }

    const next = new URLSearchParams(navigation.searchParams);
    next.delete("github_connected");
    next.delete("github_error");
    const search = next.toString();
    navigation.replace(`${navigation.pathname}${search ? `?${search}` : ""}`);
  }, [
    canManageWorkspace,
    githubBrowseConfigured,
    githubInstallations,
    githubInstallationsFetching,
    githubInstallationsPending,
    navigation,
    t,
  ]);

  const savedRepositories = workspace?.repos ?? EMPTY_REPOSITORIES;
  const draft = useMemo(() => repositories, [repositories]);
  const saveRepositories = useCallback(
    async (next: WorkspaceRepo[]) => {
      if (!workspace) return;
      const updated = await api.updateWorkspace(workspace.id, { repos: next });
      queryClient.setQueryData(
        workspaceKeys.list(),
        (old: Workspace[] | undefined) =>
          old?.map((item) => (item.id === updated.id ? updated : item)),
      );
    },
    [queryClient, workspace],
  );
  const allUrlsValid = repositories.every((repo) => repo.url.trim().length > 0);
  const autoSave = useAutoSave({
    value: draft,
    savedValue: savedRepositories,
    onSave: saveRepositories,
    onSuccess: () =>
      toast.success(t(($) => $.repositories.toast_saved), {
        id: "settings-auto-save",
      }),
    onError: (error) =>
      toast.error(
        error instanceof Error
          ? error.message
          : t(($) => $.repositories.toast_save_failed),
      ),
    enabled: !!workspace && canManageWorkspace && allUrlsValid,
    isEqual: repositoriesEqual,
  });

  const addRepository = () => {
    const url = remoteDraft.trim();
    if (!repositoryIdentity(url)) return;
    const next = repositories.some(repo => repositoryIdentity(repo.url) === repositoryIdentity(url)) ? repositories : [...repositories, {url}];
    setRepositories(next);
    autoSave.saveNow(next);
    setRemoteDraft("");
    setRemoteDialogOpen(false);
  };

  const openGitHubPicker = () => {
    setSelectedInstallationID(
      selectedInstallationID || githubInstallations[0]?.id || "",
    );
    setGitHubPickerOpen(true);
  };

  const handleGitHubAction = async () => {
    if (githubInstallations.length > 0) {
      openGitHubPicker();
      return;
    }
    setConnectingGitHub(true);
    try {
      const response = await api.getGitHubConnectURL(wsId, "repositories");
      if (!response.configured || !response.url) {
        toast.error(t(($) => $.repositories.github_not_configured));
        return;
      }
      window.open(response.url, "_blank", "noopener");
    } catch (error) {
      toast.error(
        error instanceof Error
          ? error.message
          : t(($) => $.repositories.github_connect_failed),
      );
    } finally {
      setConnectingGitHub(false);
    }
  };

  const closeGitHubPicker = () => {
    setGitHubPickerOpen(false);
    setSelectedRepositories(new Map());
    setRepositorySearch("");
  };

  const toggleGitHubRepository = (
    repository: GitHubRepository,
    checked: boolean,
  ) => {
    setSelectedRepositories((current) => {
      const next = new Map(current);
      if (checked) next.set(repository.id, repository);
      else next.delete(repository.id);
      return next;
    });
  };

  const importGitHubRepositories = () => {
    if (!allUrlsValid) {
      toast.error(t(($) => $.repositories.complete_manual_entry_first));
      return;
    }
    const additions: WorkspaceRepo[] = [];
    const known = new Set(existingRepositoryIdentities);
    for (const repository of selectedRepositories.values()) {
      const identity = repositoryIdentity(repository.clone_url);
      if (!identity || known.has(identity) || repository.archived) continue;
      known.add(identity);
      additions.push({
        url: repository.clone_url,
        ...(repository.description?.trim()
          ? { description: repository.description.trim() }
          : {}),
      });
    }
    if (additions.length === 0) {
      closeGitHubPicker();
      return;
    }
    const next = [...repositories, ...additions];
    setRepositories(next);
    autoSave.saveNow(next);
    closeGitHubPicker();
  };

  // Synchronous store work, but the promise is what holds the dialog open until
  // the row it confirms has actually gone — `useSettingsConfirm` refuses to
  // close on a non-thenable return.
  const removeRepository = async (index: number) => {
    const next = repositories.filter((_, repoIndex) => repoIndex !== index);
    setRepositories(next);
    await autoSave.saveNow(next);
  };

  const openRemoveConfirm = (index: number) =>
    confirm({
      title: t(($) => $.repositories.delete_confirm_title),
      description: t(($) => $.repositories.delete_confirm_description),
      confirmLabel: t(($) => $.repositories.delete_confirm_action),
      cancelLabel: t(($) => $.repositories.delete_confirm_cancel),
      onConfirm: () => removeRepository(index),
    });

  if (!workspace) return null;

  return (
    <div className="space-y-8">
      <SettingsGroup
        extra={
          isDesktopShell() ? (
            // `SettingsPillButton` with no `tone` — the `muted` pill, which is
            // Lobe's `fill` on a transparent border, not the bordered default.
            <Button
              icon={<FolderOpen className="size-4" />}
              shape="round"
              type="fill"
              onClick={() => useModalStore.getState().open("create-project")}
            >
              {t(($) => $.repositories.choose_local_project)}
            </Button>
          ) : undefined
        }
        title={t(($) => $.repositories.local_projects_title)}
      >
        {projects.length === 0 ? (
          <SettingsEmptyState title={t(($) => $.repositories.local_projects_empty)} />
        ) : (
          projects.map((project, index) => (
            // A project is a title with its resources under it, not a
            // label/control pair, so there is no `Form.Item` here — the retired
            // `SettingsListRow` is inlined instead of re-homed. The group panel
            // already supplies the `px-4` the old card row carried.
            <div
              key={project.id}
              className={index > 0 ? "border-t border-border pt-3" : undefined}
            >
              <div className="flex min-h-16 items-center gap-4 text-body font-medium">
                {project.title}
              </div>
              <div className="pb-3">
                <ProjectResourcesSection
                  projectId={project.id}
                  deferUntilExpanded
                />
              </div>
            </div>
          ))
        )}
      </SettingsGroup>

      {/* The autosave readout was `SettingsTab`'s `action`, which the nested
          branch returned before reading — so it rendered in the standalone unit
          tests and never in the dialog, where the tab is always mounted. It
          lives on this group's `extra` now, which renders on both branches.
          The readout tracks `workspace.repos`, so it belongs to this section
          rather than to the tab. `SettingsSaveState` renders `null` while idle,
          so the header carries nothing extra at rest.

          The other `action` in this file went to the *first* group's `extra`:
          `SettingsSection`'s `action` is read unconditionally, so that one was
          never lost and only had to be carried across. */}
      <SettingsGroup
        extra={
          <SettingsSaveState
            status={autoSave.status}
            savingLabel={t(($) => $.auto_save.saving)}
            savedLabel={t(($) => $.auto_save.saved)}
            errorLabel={t(($) => $.auto_save.failed)}
          />
        }
        title={t(($) => $.repositories.remote_projects_title)}
      >
        {repositories.length === 0 ? (
          <SettingsEmptyState title={t(($) => $.repositories.empty)} />
        ) : null}

        {repositories.map((repository, index) => (
          <SettingsFormRow
            key={index}
            align="start"
            divider={index > 0}
            label={
              <span className="flex items-center gap-2 break-all">
                <FolderGit className="size-4 shrink-0" />
                {githubShortLabel(repository.url)}
              </span>
            }
            description={
              <details className="mt-1 text-caption text-muted-foreground">
                <summary className="cursor-pointer">{t(($) => $.repositories.remote_details)}</summary>
                <p className="break-all font-mono">{repository.url}</p>
                {repository.description?.trim() ? <p className="mt-1">{repository.description}</p> : null}
              </details>
            }
          >
            {canManageWorkspace ? (
              // `SettingsPillButton tone="destructive"` — the mapping's
              // `type="fill"` + `danger`, which is the destructive fill rather
              // than the solid red a primary-danger button would give.
              <Button
                aria-label={t(($) => $.repositories.delete_aria)}
                danger
                shape="round"
                type="fill"
                onClick={() => openRemoveConfirm(index)}
              >
                {t(($) => $.repositories.delete_aria)}
              </Button>
            ) : null}
          </SettingsFormRow>
        ))}

        {canManageWorkspace ? (
          <div className="flex flex-wrap items-center justify-between gap-2 pt-3">
            <div className="flex flex-wrap items-center gap-2">
              <Button
                icon={<Plus className="size-4" />}
                shape="round"
                type="fill"
                onClick={() => setRemoteDialogOpen(true)}
              >
                {t(($) => $.repositories.add)}
              </Button>
              <Button
                disabled={
                  connectingGitHub ||
                  !githubBrowseConfigured ||
                  (!githubConnectConfigured &&
                    githubInstallations.length === 0)
                }
                icon={
                  connectingGitHub ? (
                    <LoaderCircle className="size-3.5 animate-spin" />
                  ) : (
                    <GitHubMark className="size-3.5" />
                  )
                }
                shape="round"
                title={
                  !githubBrowseConfigured
                    ? t(($) => $.repositories.github_browse_not_configured)
                    : undefined
                }
                type="primary"
                onClick={handleGitHubAction}
              >
                {githubInstallations.length > 0
                  ? t(($) => $.repositories.choose_from_github)
                  : t(($) => $.repositories.connect_github)}
              </Button>
            </div>
            {!allUrlsValid ? (
              <span className="text-caption text-muted-foreground">
                {t(($) => $.repositories.url_empty)}
              </span>
            ) : null}
          </div>
        ) : (
          <div className="py-3 text-caption text-muted-foreground">
            {t(($) => $.repositories.manage_hint)}
          </div>
        )}
      </SettingsGroup>

      <Modal
        open={remoteDialogOpen}
        title={t(($) => $.repositories.add)}
        width={512}
        footer={
          <Button
            disabled={!repositoryIdentity(remoteDraft)}
            type="primary"
            onClick={addRepository}
          >
            {t(($) => $.repositories.add)}
          </Button>
        }
        onCancel={() => setRemoteDialogOpen(false)}
      >
        <div className="flex flex-col gap-3">
          <p className="text-caption text-muted-foreground">
            {t(($) => $.repositories.remote_entry_hint)}
          </p>
          <Input
            aria-label={t(($) => $.repositories.url_placeholder)}
            placeholder={t(($) => $.repositories.url_placeholder)}
            value={remoteDraft}
            onChange={(event) => setRemoteDraft(event.target.value)}
          />
        </div>
      </Modal>

      <Modal
        open={githubPickerOpen}
        title={t(($) => $.repositories.github_picker_title)}
        width={672}
        footer={
          <div className="flex w-full items-center gap-2">
            <p className="mr-auto text-caption text-muted-foreground">
              {t(($) => $.repositories.github_selected_count, {
                count: selectedRepositories.size,
              })}
            </p>
            {/* `variant="ghost"` is Lobe's `type="text"`; the import button had
                no `variant`, i.e. shadcn's solid primary.

                Neither takes `shape="round"`. The pill geometry belongs to
                `SettingsPillButton` (reference:112), and every in-page pill in
                this file came from one; the two `DialogFooter`s were plain
                `Button`s, whose base carries `rounded-lg`. The add-remote
                footer and the "load more" row are plain `Button`s for the same
                reason. Measured: a `shape="round"` button is 999px, a plain one
                is 6px — the host dialog's own footer button, on the same
                screen, is 6px. */}
            <Button type="text" onClick={closeGitHubPicker}>
              {t(($) => $.repositories.github_cancel)}
            </Button>
            <Button
              disabled={selectedRepositories.size === 0 || !allUrlsValid}
              type="primary"
              onClick={importGitHubRepositories}
            >
              {t(($) => $.repositories.github_import)}
            </Button>
          </div>
        }
        onCancel={closeGitHubPicker}
      >
        <div className="flex flex-col gap-3">
          <p className="text-caption text-muted-foreground">
            {t(($) => $.repositories.github_picker_description)}
          </p>

          <div className="space-y-3">
            {githubInstallations.length > 1 ? (
              <SettingsSelect
                className="w-full"
                label={t(($) => $.repositories.github_account)}
                options={githubInstallations.map((installation) => ({
                  value: installation.id,
                  label: installation.account_login,
                }))}
                value={selectedInstallationID}
                onValueChange={(value) => setSelectedInstallationID(value ?? "")}
              />
            ) : githubInstallations[0] ? (
              <p className="text-caption text-muted-foreground">
                {t(($) => $.repositories.github_account)}:{" "}
                <span className="font-medium text-foreground">
                  {githubInstallations[0].account_login}
                </span>
              </p>
            ) : null}

            <SettingsSearchBar
              className="w-full"
              label={t(($) => $.repositories.github_search_placeholder)}
              placeholder={t(($) => $.repositories.github_search_placeholder)}
              value={repositorySearch}
              onValueChange={setRepositorySearch}
            />
          </div>

          <div className="max-h-[45vh] min-h-0 overflow-y-auto border-y">
            {githubRepositoriesQuery.isPending ? (
              <div className="flex items-center justify-center gap-2 px-6 py-12 text-body text-muted-foreground">
                <LoaderCircle className="size-4 animate-spin" />
                {t(($) => $.repositories.github_loading)}
              </div>
            ) : githubRepositoriesQuery.isError ? (
              <div className="px-6 py-12 text-center text-body text-muted-foreground">
                {t(($) => $.repositories.github_load_failed)}
              </div>
            ) : filteredGitHubRepositories.length === 0 ? (
              <div className="px-6 py-12 text-center text-body text-muted-foreground">
                {repositorySearch
                  ? t(($) => $.repositories.github_no_search_results)
                  : t(($) => $.repositories.github_empty)}
              </div>
            ) : (
              <div className="divide-y">
                {filteredGitHubRepositories.map((repository) => {
                  const identity = repositoryIdentity(repository.clone_url);
                  const alreadyAdded =
                    !!identity && existingRepositoryIdentities.has(identity);
                  const disabled = alreadyAdded || repository.archived;
                  return (
                    <label
                      key={repository.id}
                      htmlFor={`github-repository-${repository.id}`}
                      className="flex items-start gap-3 px-6 py-3.5"
                    >
                      {/* `onChange`, not `onCheckedChange`. This is the
                          mirror image of the `Switch` trap in the same
                          component family: `Switch` **accepts**
                          `onCheckedChange` in its type and drops it at
                          runtime; `Checkbox` **rejects** it in its type
                          (`CheckboxProps` omits the base prop and declares
                          `onChange?: (checked: boolean) => void`) while its
                          runtime destructure maps `onChange` onto
                          `onCheckedChange`. Neither name tells you which
                          behaviour you are getting — only the destructure and
                          the type of the component you imported do. */}
                      <Checkbox
                        className="mt-0.5"
                        checked={
                          alreadyAdded ||
                          selectedRepositories.has(repository.id)
                        }
                        disabled={disabled}
                        id={`github-repository-${repository.id}`}
                        onChange={(checked) =>
                          toggleGitHubRepository(repository, checked)
                        }
                      />
                      <span className="min-w-0 flex-1 space-y-1">
                        <span className="flex flex-wrap items-center gap-2">
                          <span className="truncate text-body font-medium">
                            {repository.full_name}
                          </span>
                          {repository.private ? (
                            <Badge variant="secondary">
                              {t(($) => $.repositories.github_private)}
                            </Badge>
                          ) : null}
                          {repository.archived ? (
                            <Badge variant="outline">
                              {t(($) => $.repositories.github_archived)}
                            </Badge>
                          ) : null}
                          {alreadyAdded ? (
                            <Badge variant="outline">
                              {t(($) => $.repositories.github_added)}
                            </Badge>
                          ) : null}
                        </span>
                        {repository.description ? (
                          <span className="block truncate text-caption text-muted-foreground">
                            {repository.description}
                          </span>
                        ) : null}
                      </span>
                    </label>
                  );
                })}
              </div>
            )}

            {githubRepositoriesQuery.hasNextPage ? (
              <div className="flex justify-center border-t p-3">
                {/* `variant="ghost"` maps to `type="text"`. */}
                <Button
                  disabled={githubRepositoriesQuery.isFetchingNextPage}
                  type="text"
                  onClick={() => githubRepositoriesQuery.fetchNextPage()}
                >
                  {githubRepositoriesQuery.isFetchingNextPage
                    ? t(($) => $.repositories.github_loading)
                    : t(($) => $.repositories.github_load_more)}
                </Button>
              </div>
            ) : null}
          </div>
        </div>
      </Modal>
    </div>
  );
}
