"use client";

import { useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { toast } from "sonner";
import { ExternalLink } from "lucide-react";
import { Button } from "@lobehub/ui/base-ui";
import { useAuthStore } from "@orvilo/core/auth";
import { useWorkspaceId } from "@orvilo/core/hooks";
import { useCurrentWorkspace } from "@orvilo/core/paths";
import { memberListOptions, workspaceKeys } from "@orvilo/core/workspace/queries";
import {
  deriveGitHubSettings,
  githubInstallationsOptions,
} from "@orvilo/core/github";
import { api } from "@orvilo/core/api";
import type { Workspace } from "@orvilo/core/types";
import { useNavigation } from "../../navigation";
import { useT } from "../../i18n";
import { useSettingsConfirm } from "./settings-confirm";
import { SettingsFormRow, SettingsGroup } from "./settings-shell";
import { SettingsSwitch } from "./settings-switch";
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
  // The `disconnectTarget` / `disconnecting` pair of `useState`s is gone with
  // the `AlertDialog`: the imperative confirm owns the open state and the
  // in-flight spinner, so the row hands the installation id straight through.
  const confirm = useSettingsConfirm();

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

  // Returns its promise and lets a failure reject: that is the *caller* half of
  // `confirmModal`'s contract, and it is what keeps the dialog open when the
  // request fails. `useSettingsConfirm` throws instead of closing if a call
  // site ever returns something that is not thenable.
  async function handleDisconnect(installationId: string) {
    try {
      await api.deleteGitHubInstallation(wsId, installationId);
      await qc.invalidateQueries({ queryKey: ["github", wsId] });
      toast.success(t(($) => $.github.toast_disconnected));
    } catch (e) {
      toast.error(e instanceof Error ? e.message : t(($) => $.github.toast_disconnect_failed));
      throw e;
    }
  }

  const openDisconnectConfirm = (installationId: string) =>
    confirm({
      title: t(($) => $.github.disconnect_confirm_title),
      description: t(($) => $.github.disconnect_confirm_description),
      confirmLabel: t(($) => $.github.disconnect_confirm_action),
      cancelLabel: t(($) => $.github.disconnect_confirm_cancel),
      onConfirm: () => handleDisconnect(installationId),
    });

  if (!workspace) return null;

  const repositoriesHref = `${navigation.pathname}?tab=repositories`;

  return (
    <div className="space-y-8">
      {/* The baseline had a bordered card with no heading and one switch row.
          `Form.Group` always renders a header, so a titleless outlined group
          would have drawn the ~59px dead header `tokens-tab` measured — the
          section's own name is therefore the group title, and the row keeps
          only the description. Same strings, same order, no new copy. */}
      <SettingsGroup title={t(($) => $.github.section_master)}>
        <SettingsFormRow
          align="start"
          description={
            flags.enabled
              ? t(($) => $.github.master_description_on)
              : t(($) => $.github.master_description_off)
          }
        >
          <SettingsSwitch
            checked={flags.enabled}
            disabled={!canManage || savingKey === "github_enabled"}
            id="github-master"
            label={t(($) => $.github.section_master)}
            onCheckedChange={(v) => persistSetting("github_enabled", v)}
          />
        </SettingsFormRow>
      </SettingsGroup>

      <SettingsGroup title={t(($) => $.github.section_connection)}>
        <SettingsFormRow
          align="start"
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
        >
          {canManage ? (
            connected && primaryInstallation ? (
              // `SettingsPillButton` with no `tone` and no `active` was the
              // `muted` pill: `bg-muted` on a transparent border, which is
              // Lobe's `fill`, not its bordered default. `shape="round"`
              // carries the pill geometry.
              <Button
                shape="round"
                type="fill"
                onClick={() => openDisconnectConfirm(primaryInstallation.id)}
              >
                {t(($) => $.github.disconnect)}
              </Button>
            ) : (
              // `active` resolved `SettingsPillButton`'s tone to `primary`.
              <Button
                disabled={connecting || !configured}
                shape="round"
                title={
                  !configured
                    ? t(($) => $.github.connect_disabled_tooltip)
                    : undefined
                }
                type="primary"
                onClick={handleConnect}
              >
                {connecting
                  ? t(($) => $.github.connect_opening)
                  : t(($) => $.github.connect_github)}
              </Button>
            )
          ) : null}
        </SettingsFormRow>

        {/* The group panel already carries `padding-inline: 16px`
            (`Collapse`'s `DEFAULT_PADDING`), so these notices lost the `px-4`
            that lined them up with the old card's rows and keep only the
            block padding. */}
        {canManage && !configured ? (
          <p className="py-3 text-caption text-muted-foreground">
            {t(($) => $.github.not_configured)}{" "}
            <code className="rounded bg-muted px-1 py-0.5 text-micro">GITHUB_APP_SLUG</code>{" "}
            {t(($) => $.github.not_configured_and)}{" "}
            <code className="rounded bg-muted px-1 py-0.5 text-micro">GITHUB_WEBHOOK_SECRET</code>.
          </p>
        ) : null}

        {!canManage && connected ? (
          <p className="py-3 text-caption text-muted-foreground">
            {t(($) => $.github.read_only_hint)}
          </p>
        ) : null}
      </SettingsGroup>

      <SettingsGroup title={t(($) => $.github.section_features)}>
        <SettingsFormRow
          align="start"
          description={t(($) => $.github.feature_pr_sidebar_description)}
          label={t(($) => $.github.feature_pr_sidebar_label)}
        >
          <SettingsSwitch
            checked={flags.prSidebar}
            disabled={!canManage || !flags.enabled || savingKey === "github_pr_sidebar_enabled"}
            id="github-pr-sidebar"
            label={t(($) => $.github.feature_pr_sidebar_label)}
            onCheckedChange={(v) => persistSetting("github_pr_sidebar_enabled", v)}
          />
        </SettingsFormRow>

        <SettingsFormRow
          align="start"
          description={
            <>
              {t(($) => $.github.feature_co_author_description_prefix)}{" "}
              <code className="rounded bg-muted px-1 py-0.5 text-caption">
                {"Co-authored-by: orvilo-agent <github@aspectlylabs.com>"}
              </code>{" "}
              {t(($) => $.github.feature_co_author_description_suffix)}
            </>
          }
          label={t(($) => $.github.feature_co_author_label)}
        >
          <SettingsSwitch
            checked={flags.coAuthor}
            disabled={!canManage || !flags.enabled || savingKey === "co_authored_by_enabled"}
            id="github-coauthor"
            label={t(($) => $.github.feature_co_author_label)}
            onCheckedChange={(v) => persistSetting("co_authored_by_enabled", v)}
          />
        </SettingsFormRow>

        <SettingsFormRow
          align="start"
          description={t(($) => $.github.feature_auto_link_description)}
          label={t(($) => $.github.feature_auto_link_label)}
        >
          <SettingsSwitch
            checked={flags.autoLinkPRs}
            disabled={!canManage || !flags.enabled || savingKey === "github_auto_link_prs_enabled"}
            id="github-auto-link"
            label={t(($) => $.github.feature_auto_link_label)}
            onCheckedChange={(v) => persistSetting("github_auto_link_prs_enabled", v)}
          />
        </SettingsFormRow>
      </SettingsGroup>

      <SettingsGroup title={t(($) => $.github.section_repositories)}>
        <SettingsFormRow label={t(($) => $.github.repositories_shortcut_label)}>
          {/* `SettingsPillButton` with no `tone` again — the `muted` pill, i.e.
              Lobe's `fill`. `icon` takes the node directly, the way the other
              migrated tabs pass it. */}
          <Button
            icon={<ExternalLink className="size-4" />}
            shape="round"
            type="fill"
            onClick={() => navigation.push(repositoriesHref)}
          >
            {t(($) => $.github.repositories_shortcut_link)}
          </Button>
        </SettingsFormRow>
      </SettingsGroup>
    </div>
  );
}
