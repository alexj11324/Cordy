"use client";

import { useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { toast } from "sonner";
import { ExternalLink } from "lucide-react";
import { Switch } from "@patchbay/ui/components/ui/switch";
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
import { useAuthStore } from "@patchbay/core/auth";
import { useWorkspaceId } from "@patchbay/core/hooks";
import { useCurrentWorkspace } from "@patchbay/core/paths";
import { memberListOptions, workspaceKeys } from "@patchbay/core/workspace/queries";
import {
  deriveGitHubSettings,
  githubInstallationsOptions,
} from "@patchbay/core/github";
import { api } from "@patchbay/core/api";
import type { Workspace } from "@patchbay/core/types";
import { useNavigation } from "../../navigation";
import { useT } from "../../i18n";
import {
  SettingsCard,
  SettingsPillButton,
  SettingsRow,
  SettingsSection,
  SettingsTab,
} from "./settings-layout";
import { GitHubMark } from "./github-mark";

type SettingsKey =
  | "github_enabled"
  | "github_pr_sidebar_enabled"
  | "co_authored_by_enabled"
  | "github_auto_link_prs_enabled";

export function GitHubTab() {
  const { t } = useT("settings");
  const workspace = useCurrentWorkspace();
  const wsId = useWorkspaceId();
  const qc = useQueryClient();
  const navigation = useNavigation();
  const user = useAuthStore((s) => s.user);

  const { data: members = [] } = useQuery(memberListOptions(wsId));
  const currentMember = members.find((m) => m.user_id === user?.id) ?? null;
  // `canView` gates the read-only installation list (every workspace member
  // sees it after MUL-2413); `canManage` gates the Connect / Disconnect
  // actions and comes from the backend response (`can_manage`) so the
  // frontend never claims management rights the server would reject.
  const canView = !!currentMember;

  const { data: installationData } = useQuery({
    ...githubInstallationsOptions(wsId),
    enabled: !!wsId && canView,
  });
  const installations = installationData?.installations ?? [];
  const configured = installationData?.configured ?? false;
  const canManage = installationData?.can_manage === true;
  const connected = installations.length > 0;
  const primaryInstallation = installations[0] ?? null;

  const flags = deriveGitHubSettings(workspace);
  const [savingKey, setSavingKey] = useState<SettingsKey | null>(null);
  const [connecting, setConnecting] = useState(false);
  const [disconnectTarget, setDisconnectTarget] = useState<string | null>(null);
  const [disconnecting, setDisconnecting] = useState(false);

  async function persistSetting(key: SettingsKey, next: boolean) {
    if (!workspace || savingKey) return;
    setSavingKey(key);
    try {
      const merged = {
        ...((workspace.settings as Record<string, unknown>) ?? {}),
        [key]: next,
      };
      const updated = await api.updateWorkspace(workspace.id, { settings: merged });
      qc.setQueryData(workspaceKeys.list(), (old: Workspace[] | undefined) =>
        old?.map((ws) => (ws.id === updated.id ? updated : ws)),
      );
      toast.success(t(($) => $.auto_save.toast_saved), {
        id: "settings-auto-save",
      });
    } catch (e) {
      toast.error(e instanceof Error ? e.message : t(($) => $.github.toast_failed));
    } finally {
      setSavingKey(null);
    }
  }

  async function handleConnect() {
    setConnecting(true);
    try {
      const resp = await api.getGitHubConnectURL(wsId);
      if (!resp.configured || !resp.url) {
        toast.error(t(($) => $.github.toast_not_configured));
        return;
      }
      window.open(resp.url, "_blank", "noopener");
    } catch (e) {
      toast.error(e instanceof Error ? e.message : t(($) => $.github.toast_open_failed));
    } finally {
      setConnecting(false);
    }
  }

  async function handleDisconnect() {
    if (!disconnectTarget || disconnecting) return;
    setDisconnecting(true);
    try {
      await api.deleteGitHubInstallation(wsId, disconnectTarget);
      await qc.invalidateQueries({ queryKey: ["github", wsId] });
      toast.success(t(($) => $.github.toast_disconnected));
      setDisconnectTarget(null);
    } catch (e) {
      toast.error(e instanceof Error ? e.message : t(($) => $.github.toast_disconnect_failed));
    } finally {
      setDisconnecting(false);
    }
  }

  if (!workspace) return null;

  const repositoriesHref = `${navigation.pathname}?tab=repositories`;

  return (
    <SettingsTab
      title={t(($) => $.page.tabs.github)}
      description={t(($) => $.github.page_description)}
    >
      <SettingsSection>
        <SettingsCard>
          <SettingsRow
            htmlFor="github-master"
            label={t(($) => $.github.section_master)}
            description={
              flags.enabled
                ? t(($) => $.github.master_description_on)
                : t(($) => $.github.master_description_off)
            }
            align="start"
          >
            <Switch
              id="github-master"
              checked={flags.enabled}
              onCheckedChange={(v) => persistSetting("github_enabled", v)}
              disabled={!canManage || savingKey === "github_enabled"}
            />
          </SettingsRow>
        </SettingsCard>
      </SettingsSection>

      <SettingsSection title={t(($) => $.github.section_connection)}>
        <SettingsCard>
          <SettingsRow
            label={
              <span className="inline-flex items-center gap-2">
                <GitHubMark className="size-4" />
                {t(($) => $.github.connection_title)}
              </span>
            }
            description={
              connected ? (
                <>
                  {t(($) => $.github.connected_to, {
                    login: installations.map((i) => i.account_login).join(", "),
                  })}
                  {primaryInstallation?.connected_by ? (
                    <span className="mt-1 block">
                      {t(($) => $.github.connected_by, {
                        name: primaryInstallation.connected_by!,
                      })}
                    </span>
                  ) : null}
                </>
              ) : canManage ? (
                <>
                  {t(($) => $.github.connection_description_prefix)}{" "}
                  <code className="rounded bg-muted px-1 py-0.5 text-micro">
                    {t(($) => $.github.connection_identifier_example)}
                  </code>{" "}
                  {t(($) => $.github.connection_description_suffix)}{" "}
                  <strong>{t(($) => $.github.connection_description_done)}</strong>.
                </>
              ) : (
                t(($) => $.github.contact_admin_to_connect)
              )
            }
            align="start"
          >
            {canManage ? (
              connected && primaryInstallation ? (
                <SettingsPillButton
                  onClick={() => setDisconnectTarget(primaryInstallation.id)}
                >
                  {t(($) => $.github.disconnect)}
                </SettingsPillButton>
              ) : (
                <SettingsPillButton
                  active
                  onClick={handleConnect}
                  disabled={connecting || !configured}
                  title={
                    !configured
                      ? t(($) => $.github.connect_disabled_tooltip)
                      : undefined
                  }
                >
                  {connecting
                    ? t(($) => $.github.connect_opening)
                    : t(($) => $.github.connect_github)}
                </SettingsPillButton>
              )
            ) : null}
          </SettingsRow>

          {canManage && !configured ? (
            <p className="px-4 py-3 text-caption text-muted-foreground">
              {t(($) => $.github.not_configured)}{" "}
              <code className="rounded bg-muted px-1 py-0.5 text-micro">GITHUB_APP_SLUG</code>{" "}
              {t(($) => $.github.not_configured_and)}{" "}
              <code className="rounded bg-muted px-1 py-0.5 text-micro">GITHUB_WEBHOOK_SECRET</code>.
            </p>
          ) : null}

          {!canManage && connected ? (
            <p className="px-4 py-3 text-caption text-muted-foreground">
              {t(($) => $.github.read_only_hint)}
            </p>
          ) : null}
        </SettingsCard>
      </SettingsSection>

      <SettingsSection title={t(($) => $.github.section_features)}>
        <SettingsCard>
          <SettingsRow
            htmlFor="github-pr-sidebar"
            label={t(($) => $.github.feature_pr_sidebar_label)}
            description={t(($) => $.github.feature_pr_sidebar_description)}
            align="start"
          >
            <Switch
              id="github-pr-sidebar"
              checked={flags.prSidebar}
              disabled={!canManage || !flags.enabled || savingKey === "github_pr_sidebar_enabled"}
              onCheckedChange={(v) => persistSetting("github_pr_sidebar_enabled", v)}
            />
          </SettingsRow>

          <SettingsRow
            htmlFor="github-coauthor"
            label={t(($) => $.github.feature_co_author_label)}
            description={
              <>
                {t(($) => $.github.feature_co_author_description_prefix)}{" "}
                <code className="rounded bg-muted px-1 py-0.5 text-caption">
                  {"Co-authored-by: patchbay-agent <github@patchbay.ai>"}
                </code>{" "}
                {t(($) => $.github.feature_co_author_description_suffix)}
              </>
            }
            align="start"
          >
            <Switch
              id="github-coauthor"
              checked={flags.coAuthor}
              disabled={!canManage || !flags.enabled || savingKey === "co_authored_by_enabled"}
              onCheckedChange={(v) => persistSetting("co_authored_by_enabled", v)}
            />
          </SettingsRow>

          <SettingsRow
            htmlFor="github-auto-link"
            label={t(($) => $.github.feature_auto_link_label)}
            description={t(($) => $.github.feature_auto_link_description)}
            align="start"
          >
            <Switch
              id="github-auto-link"
              checked={flags.autoLinkPRs}
              disabled={!canManage || !flags.enabled || savingKey === "github_auto_link_prs_enabled"}
              onCheckedChange={(v) => persistSetting("github_auto_link_prs_enabled", v)}
            />
          </SettingsRow>
        </SettingsCard>
      </SettingsSection>

      <SettingsSection title={t(($) => $.github.section_repositories)}>
        <SettingsCard>
          <SettingsRow label={t(($) => $.github.repositories_shortcut_label)}>
            <SettingsPillButton
              icon={ExternalLink}
              onClick={() => navigation.push(repositoriesHref)}
            >
              {t(($) => $.github.repositories_shortcut_link)}
            </SettingsPillButton>
          </SettingsRow>
        </SettingsCard>
      </SettingsSection>

      <AlertDialog
        open={!!disconnectTarget}
        onOpenChange={(v) => {
          if (!v && !disconnecting) setDisconnectTarget(null);
        }}
      >
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>
              {t(($) => $.github.disconnect_confirm_title)}
            </AlertDialogTitle>
            <AlertDialogDescription>
              {t(($) => $.github.disconnect_confirm_description)}
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel disabled={disconnecting}>
              {t(($) => $.github.disconnect_confirm_cancel)}
            </AlertDialogCancel>
            <AlertDialogAction onClick={handleDisconnect} disabled={disconnecting}>
              {disconnecting
                ? t(($) => $.github.disconnecting)
                : t(($) => $.github.disconnect_confirm_action)}
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </SettingsTab>
  );
}
