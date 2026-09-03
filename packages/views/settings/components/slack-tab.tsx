"use client";

import { MessagingConnectionStatus } from "./messaging-connection-status";

import { useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { toast } from "sonner";
import { ChevronRight, ExternalLink, Trash2 } from "lucide-react";
import { SlackMark } from "./slack-mark";
import { cn } from "@patchbay/ui/lib/utils";
import { Button } from "@patchbay/ui/components/ui/button";
import { Card, CardContent } from "@patchbay/ui/components/ui/card";
import {
  Dialog,
  DialogContent,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@patchbay/ui/components/ui/dialog";
import { Input } from "@patchbay/ui/components/ui/input";
import { Label } from "@patchbay/ui/components/ui/label";
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
import { memberListOptions } from "@patchbay/core/workspace/queries";
import { useActorName } from "@patchbay/core/workspace/hooks";
import { slackInstallationsOptions, slackKeys } from "@patchbay/core/slack";
import { api } from "@patchbay/core/api";
import type { SlackInstallation } from "@patchbay/core/types";
import { ActorAvatar } from "../../common/actor-avatar";
import { openExternal } from "../../platform";
import { useLocale, useT } from "../../i18n";

// SlackTab is the workspace settings panel for Slack bot installations.
// Listing is member-visible; the disconnect action is admin-only (the backend
// enforces it; the UI hides the button for non-admins to match).
//
// Adding a new installation flows through the Agent detail page: the install
// path is per-agent (each Patchbay agent gets exactly one bot — the
// (workspace_id, agent_id, channel_type) UNIQUE in channel_installation), so
// asking the user to pick an agent here would re-create that page's picker.
export function SlackTab() {
  const { t } = useT("settings");
  const wsId = useWorkspaceId();
  const qc = useQueryClient();
  const user = useAuthStore((s) => s.user);

  const { data: members = [] } = useQuery(memberListOptions(wsId));
  const currentMember = members.find((m) => m.user_id === user?.id) ?? null;
  const canManage =
    currentMember?.role === "owner" || currentMember?.role === "admin";

  const { data, isLoading, isError, refetch } = useQuery({
    ...slackInstallationsOptions(wsId),
    enabled: !!wsId,
  });
  const installations = (data?.installations ?? []).map((installation) =>
    isError ? { ...installation, runtime: undefined } : installation,
  );
  const configured = data?.configured === true;
  // install_supported tracks whether the OAuth client credentials are wired on
  // the server. When false, "Connect Slack" would 503, so we hide the connect
  // entry points and surface a "coming soon" notice. Already-installed bots
  // still appear below and remain manageable.
  const installSupported = data?.install_supported === true;
  // managed_supported tracks the hosted-OAuth begin path specifically (service
  // wired + client credentials set). The workspace-level Connect button below
  // shows only on this flag; BYO installs keep their own per-agent dialog.
  const managedSupported = data?.managed_supported === true;
  const installedManagedBot = installations.find(
    (inst) => isWorkspaceInstall(inst.agent_id) && inst.status === "installed",
  );

  const [disconnectTarget, setDisconnectTarget] = useState<string | null>(null);
  const [disconnecting, setDisconnecting] = useState(false);
  const [managedConnecting, setManagedConnecting] = useState(false);

  async function handleManagedConnect() {
    if (managedConnecting) return;
    setManagedConnecting(true);
    try {
      // Land back on this settings tab after the callback 302s: the install
      // list revalidates and the new bot appears.
      const response = await api.beginManagedSlackInstall(wsId, window.location.href);
      if (!response.authorize_url) {
        throw new Error("Slack authorization URL was empty");
      }
      window.location.assign(response.authorize_url);
    } catch (e) {
      toast.error(
        e instanceof Error ? e.message : t(($) => $.slack.managed_failed_toast),
      );
      setManagedConnecting(false);
    }
  }

  async function handleDisconnect() {
    if (!disconnectTarget || disconnecting) return;
    setDisconnecting(true);
    try {
      await api.deleteSlackInstallation(wsId, disconnectTarget);
      await qc.invalidateQueries({ queryKey: slackKeys.installations(wsId) });
      toast.success(t(($) => $.slack.toast_disconnected));
      setDisconnectTarget(null);
    } catch (e) {
      toast.error(
        e instanceof Error ? e.message : t(($) => $.slack.toast_disconnect_failed),
      );
    } finally {
      setDisconnecting(false);
    }
  }

  if (isError && !data) {
    return (
      <div role="alert" className="space-y-2">
        <p className="text-caption text-muted-foreground">
          {t(($) => $.page.connection_status.unavailable)}
        </p>
        <Button variant="outline" size="sm" onClick={() => void refetch()}>
          {t(($) => $.page.connection_status.retry)}
        </Button>
      </div>
    );
  }

  return (
    <div className="space-y-8">
      {!configured ? (
        <Card>
          <CardContent className="space-y-2">
            <p className="text-body font-medium">{t(($) => $.slack.not_enabled_title)}</p>
            <p className="text-caption text-muted-foreground">
              {t(($) => $.slack.not_enabled_description_prefix)}{" "}
              <code className="rounded bg-muted px-1 py-0.5 text-micro">
                PATCHBAY_SLACK_SECRET_KEY
              </code>{" "}
              {t(($) => $.slack.not_enabled_description_suffix)}{" "}
              {t(($) => $.slack.not_enabled_self_host_hint)}
            </p>
          </CardContent>
        </Card>
      ) : !installSupported && installations.length === 0 ? (
        <Card>
          <CardContent className="space-y-2">
            <p className="text-body font-medium">{t(($) => $.slack.preview_title)}</p>
            <p className="text-caption text-muted-foreground">
              {t(($) => $.slack.preview_description)}
            </p>
          </CardContent>
        </Card>
      ) : (
        <>
          {canManage && managedSupported && !installedManagedBot ? (
            <Card>
              <CardContent className="flex flex-wrap items-center justify-between gap-3">
                <div className="space-y-1">
                  <p className="text-body font-medium">
                    {t(($) => $.slack.managed_connect_title)}
                  </p>
                  <p className="text-caption text-muted-foreground">
                    {t(($) => $.slack.managed_connect_description)}
                  </p>
                </div>
                <Button
                  variant="outline"
                  size="sm"
                  onClick={handleManagedConnect}
                  disabled={managedConnecting}
                  data-testid="slack-managed-connect"
                >
                  <SlackMark className="h-3 w-3" />
                  {managedConnecting
                    ? t(($) => $.slack.managed_connecting)
                    : t(($) => $.slack.managed_connect_button)}
                </Button>
              </CardContent>
            </Card>
          ) : null}
          <section className="space-y-3">
          <h2 className="text-body font-semibold">{t(($) => $.slack.installed_bots)}</h2>
          {isLoading ? (
            <Card>
              <CardContent>
                <p className="text-body text-muted-foreground">{t(($) => $.slack.loading)}</p>
              </CardContent>
            </Card>
          ) : installations.length === 0 ? (
            <Card>
              <CardContent className="space-y-2">
                <p className="text-body font-medium">{t(($) => $.slack.empty_title)}</p>
                <p className="text-caption text-muted-foreground">
                  {t(($) => $.slack.empty_description_prefix)}{" "}
                  <strong>{t(($) => $.slack.empty_description_cta)}</strong>{" "}
                  {t(($) => $.slack.empty_description_suffix)}
                </p>
              </CardContent>
            </Card>
          ) : (
            <Card>
              <CardContent className="divide-y">
                {installations.map((inst) => (
                  <InstallationRow
                    key={inst.id}
                    installation={inst}
                    canManage={canManage}
                    onDisconnect={() => setDisconnectTarget(inst.id)}
                  />
                ))}
              </CardContent>
            </Card>
          )}
          </section>
        </>
      )}

      <AlertDialog
        open={!!disconnectTarget}
        onOpenChange={(v) => {
          if (!v && !disconnecting) setDisconnectTarget(null);
        }}
      >
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>
              {t(($) => $.slack.disconnect_confirm_title)}
            </AlertDialogTitle>
            <AlertDialogDescription>
              {t(($) => $.slack.disconnect_confirm_description)}
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel disabled={disconnecting}>
              {t(($) => $.slack.disconnect_confirm_cancel)}
            </AlertDialogCancel>
            <AlertDialogAction onClick={handleDisconnect} disabled={disconnecting}>
              {disconnecting
                ? t(($) => $.slack.disconnecting)
                : t(($) => $.slack.disconnect)}
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </div>
  );
}

function InstallationRow({
  installation,
  canManage,
  onDisconnect,
}: {
  installation: SlackInstallation;
  canManage: boolean;
  onDisconnect: () => void;
}) {
  const { t } = useT("settings");
  const locale = useLocale();
  const { getAgentName } = useActorName();
  const isInstalled = installation.status === "installed";
  // Workspace-level (managed) installs belong to no agent: show the Slack
  // workspace they connect instead of an agent name that does not exist.
  const isManaged = isWorkspaceInstall(installation.agent_id);
  const agentName = isManaged ? null : getAgentName(installation.agent_id);
  const title = isManaged
    ? t(($) => $.slack.managed_workspace_name, {
        team: installation.team_id || installation.bot_user_id,
      })
    : agentName;
  return (
    <div className="flex items-start justify-between gap-4 py-3 first:pt-0 last:pb-0">
      <div className="flex items-start gap-3">
        {isManaged ? (
          <span className="flex size-8 items-center justify-center rounded-md bg-muted">
            <SlackMark className="h-4 w-4" />
          </span>
        ) : (
          <ActorAvatar
            actorType="agent"
            actorId={installation.agent_id}
            size="lg"
            enableHoverCard
            profileLink
          />
        )}
        <div className="space-y-1">
          <MessagingConnectionStatus installation={installation} />
          <p className="text-body font-medium">
            {title}
            {!isInstalled && (
              <span className="ml-2 rounded bg-muted px-1.5 py-0.5 text-micro text-muted-foreground">
                {t(($) => $.slack.revoked_badge)}
              </span>
            )}
          </p>
          <p className="text-micro text-muted-foreground">
            {t(($) => $.slack.installed_at_label, {
              when: new Date(installation.installed_at).toLocaleString(locale),
            })}
          </p>
        </div>
      </div>
      {canManage && isInstalled && (
        <Button variant="outline" size="sm" onClick={onDisconnect}>
          <Trash2 className="h-3 w-3" />
          {t(($) => $.slack.disconnect)}
        </Button>
      )}
    </div>
  );
}

// NIL_AGENT_ID is the all-zero UUID the backend stores on workspace-level
// (managed, team-keyed) installs, which belong to no single agent — unlike BYO
// rows keyed by (workspace, agent). Rows carrying it render as the workspace's
// Slack connection, not as an agent's bot.
const NIL_AGENT_ID = "00000000-0000-0000-0000-000000000000";

function isWorkspaceInstall(agentId: string | null | undefined): boolean {
  return !agentId || agentId === NIL_AGENT_ID;
}

// SLACK_BYO_VIDEO_URL is the optional setup-tutorial video linked from the
// connect dialog. Leave "" to hide the link; set it once the walkthrough that
// shows how to create the Slack app + copy its two tokens is recorded.
const SLACK_BYO_VIDEO_URL = "";

// slackDocsUrl points at the Slack integration guide on the docs site,
// localized to the viewer's language. The docs site uses /<lang>/ path
// prefixes (English has none), matching the convention used elsewhere in the
// app for doc links (e.g. the automations webhook docs link).
function slackDocsUrl(lang: string | undefined): string {
  const prefix = lang?.startsWith("zh")
    ? "/zh"
    : lang?.startsWith("ja")
      ? "/ja"
      : lang?.startsWith("ko")
        ? "/ko"
        : "";
  return `https://patchbay.aspectlylabs.com/docs${prefix}/slack-bot-integration`;
}

// SlackAgentBindButton is the per-agent CTA exposed from the agent detail page.
// Slack uses the bring-your-own-app model: the button opens a dialog where the
// admin pastes the bot token (xoxb-) + app-level token (xapp-) of the Slack app
// they created (the backend validates both belong to the same app). Visibility:
//   1. Non-owner/admin viewers see nothing (the backend gates install/revoke).
//   2. If this agent already has an installed installation, show the connected
//      badge (already-installed bots stay manageable).
//   3. Otherwise the Connect CTA shows whenever install is available.
export function SlackAgentBindButton({
  agentId,
  agentName,
  className,
  onShowConnectedDetails,
}: {
  agentId: string;
  agentName?: string;
  className?: string;
  /**
   * When set, the connected state renders as a compact read-only status row
   * that invokes this callback on click instead of the full badge with inline
   * actions — the agent inspector passes a "jump to the Integrations tab"
   * handler so management actions live in one place.
   */
  onShowConnectedDetails?: () => void;
}) {
  const { t, i18n } = useT("settings");
  const wsId = useWorkspaceId();
  const qc = useQueryClient();
  const user = useAuthStore((s) => s.user);

  const [dialogOpen, setDialogOpen] = useState(false);
  const [botToken, setBotToken] = useState("");
  const [appToken, setAppToken] = useState("");
  const [submitting, setSubmitting] = useState(false);

  const { data: listing, isError: installationQueryFailed } = useQuery({
    ...slackInstallationsOptions(wsId),
    enabled: !!wsId,
  });
  const installSupported = listing?.install_supported === true;

  const { data: members = [] } = useQuery({
    ...memberListOptions(wsId),
    enabled: !!wsId,
  });
  const currentMember = members.find((m) => m.user_id === user?.id) ?? null;
  const canManage =
    currentMember?.role === "owner" || currentMember?.role === "admin";

  if (!canManage) return null;

  const recordedInstallation = listing?.installations.find(
    (inst) => inst.agent_id === agentId && inst.status === "installed",
  );
  const existing = recordedInstallation && installationQueryFailed
    ? { ...recordedInstallation, runtime: undefined }
    : recordedInstallation;
  if (existing) {
    return onShowConnectedDetails ? (
      <SlackAgentBotStatusRow
        installation={existing}
        onClick={onShowConnectedDetails}
        className={className}
      />
    ) : (
      <SlackAgentBotInstalledControls installation={existing} className={className} />
    );
  }

  if (!installSupported) return null;

  function closeDialog() {
    if (submitting) return;
    setDialogOpen(false);
    setBotToken("");
    setAppToken("");
  }

  async function handleSubmit() {
    const bot_token = botToken.trim();
    const app_token = appToken.trim();
    if (submitting || !agentId || !bot_token || !app_token) return;
    setSubmitting(true);
    try {
      await api.registerSlackBYO(wsId, agentId, { bot_token, app_token });
      // The slack_installation realtime event also refreshes this list, but
      // invalidate explicitly so the installed controls appear immediately.
      await qc.invalidateQueries({ queryKey: slackKeys.installations(wsId) });
      toast.success(t(($) => $.slack.byo_success_toast));
      setDialogOpen(false);
      setBotToken("");
      setAppToken("");
    } catch (e) {
      toast.error(
        e instanceof Error ? e.message : t(($) => $.slack.byo_failed_toast),
      );
    } finally {
      setSubmitting(false);
    }
  }

  const canSubmit =
    botToken.trim() !== "" && appToken.trim() !== "" && !submitting;

  return (
    <div
      className={cn("flex flex-wrap items-center gap-2", className)}
      data-testid="slack-agent-bind-buttons"
    >
      <Button
        variant="outline"
        size="sm"
        onClick={() => setDialogOpen(true)}
        disabled={!agentId}
        title={
          agentName
            ? t(($) => $.slack.bind_button_title, { agent: agentName })
            : undefined
        }
        data-testid="slack-agent-connect"
      >
        <SlackMark className="h-3 w-3" />
        {t(($) => $.slack.bind_button)}
      </Button>

      <Dialog
        open={dialogOpen}
        onOpenChange={(v) => (v ? setDialogOpen(true) : closeDialog())}
      >
        <DialogContent className="sm:max-w-lg" data-testid="slack-byo-dialog">
          <DialogHeader>
            <DialogTitle>{t(($) => $.slack.byo_dialog_title)}</DialogTitle>
          </DialogHeader>

          {SLACK_BYO_VIDEO_URL ? (
            <button
              type="button"
              onClick={() => openExternal(SLACK_BYO_VIDEO_URL)}
              className="inline-flex w-fit items-center gap-2 text-body font-medium text-primary underline-offset-2 hover:underline"
            >
              <ExternalLink className="h-4 w-4" />
              {t(($) => $.slack.byo_video_cta)}
            </button>
          ) : null}

          <button
            type="button"
            onClick={() => openExternal(slackDocsUrl(i18n.language))}
            className="inline-flex w-fit items-center gap-2 text-body font-medium text-primary underline-offset-2 hover:underline"
            data-testid="slack-byo-docs-link"
          >
            <ExternalLink className="h-4 w-4" />
            {t(($) => $.slack.byo_docs_link)}
          </button>

          <div className="space-y-4">
            <div className="space-y-1.5">
              <Label htmlFor="slack-byo-bot-token">
                {t(($) => $.slack.byo_bot_token_label)}
              </Label>
              <Input
                id="slack-byo-bot-token"
                data-testid="slack-byo-bot-token"
                value={botToken}
                onChange={(e) => setBotToken(e.target.value)}
                // Slack token prefix: a format hint, not copy.
                // eslint-disable-next-line no-restricted-syntax
                placeholder="xoxb-…"
                autoComplete="off"
                spellCheck={false}
                disabled={submitting}
              />
            </div>

            <div className="space-y-1.5">
              <Label htmlFor="slack-byo-app-token">
                {t(($) => $.slack.byo_app_token_label)}
              </Label>
              <Input
                id="slack-byo-app-token"
                data-testid="slack-byo-app-token"
                value={appToken}
                onChange={(e) => setAppToken(e.target.value)}
                // Slack token prefix: a format hint, not copy.
                // eslint-disable-next-line no-restricted-syntax
                placeholder="xapp-…"
                autoComplete="off"
                spellCheck={false}
                disabled={submitting}
              />
            </div>
          </div>

          <DialogFooter>
            <Button
              variant="outline"
              size="sm"
              onClick={closeDialog}
              disabled={submitting}
            >
              {t(($) => $.slack.byo_cancel)}
            </Button>
            <Button
              size="sm"
              onClick={handleSubmit}
              disabled={!canSubmit}
              data-testid="slack-byo-submit"
            >
              {submitting
                ? t(($) => $.slack.byo_submitting)
                : t(($) => $.slack.byo_submit)}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </div>
  );
}

// SlackAgentBotStatusRow is the compact, read-only installation affordance the
// agent inspector renders instead of the full badge; it deep-links into the
// Integrations tab where Manage / Disconnect live.
function SlackAgentBotStatusRow({
  installation,
  onClick,
  className,
}: {
  installation: SlackInstallation;
  onClick: () => void;
  className?: string;
}) {
  const { t } = useT("settings");
  return (
    <button
      type="button"
      onClick={onClick}
      className={cn(
        "flex w-full items-center gap-2 rounded-md px-2 py-1.5 text-left text-caption text-muted-foreground transition-colors hover:bg-muted focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring/50",
        className,
      )}
      data-testid="slack-agent-bot-status"
    >
      <span className="truncate">{t(($) => $.slack.section_title)}</span>
      <MessagingConnectionStatus installation={installation} compact />
      <ChevronRight className="ml-auto h-3.5 w-3.5 shrink-0" />
    </button>
  );
}

// SlackAgentBotInstalledControls is the full installed-bot affordance the
// Integrations tab renders in place of the Connect button. Two rows: status +
// soft-destructive Disconnect, then a secondary "Open in Slack" link to the
// installed workspace. Only owners/admins ever reach this component.
function SlackAgentBotInstalledControls({
  installation,
  className,
}: {
  installation: SlackInstallation;
  className?: string;
}) {
  const { t } = useT("settings");
  const wsId = useWorkspaceId();
  const qc = useQueryClient();

  const [confirmOpen, setConfirmOpen] = useState(false);
  const [disconnecting, setDisconnecting] = useState(false);

  async function handleDisconnect() {
    if (disconnecting) return;
    setDisconnecting(true);
    try {
      await api.deleteSlackInstallation(wsId, installation.id);
      await qc.invalidateQueries({ queryKey: slackKeys.installations(wsId) });
      toast.success(t(($) => $.slack.toast_disconnected));
      setConfirmOpen(false);
    } catch (e) {
      toast.error(
        e instanceof Error ? e.message : t(($) => $.slack.toast_disconnect_failed),
      );
    } finally {
      setDisconnecting(false);
    }
  }

  return (
    <div
      className={cn("space-y-2", className)}
      data-testid="slack-agent-bot-installed"
    >
      <div className="flex items-center justify-between gap-3">
        <span className="inline-flex min-w-0 items-center gap-2 text-caption text-muted-foreground">
          <MessagingConnectionStatus installation={installation} compact />
        </span>
        <Button
          variant="destructive"
          size="sm"
          onClick={() => setConfirmOpen(true)}
          disabled={disconnecting}
          title={t(($) => $.slack.agent_bot_disconnect_tooltip)}
          aria-label={t(($) => $.slack.disconnect)}
          data-testid="slack-agent-bot-disconnect"
        >
          <Trash2 className="h-3 w-3" />
          {disconnecting
            ? t(($) => $.slack.disconnecting)
            : t(($) => $.slack.disconnect)}
        </Button>
      </div>

      {installation.team_id && (
        <button
          type="button"
          onClick={() =>
            openExternal(`https://app.slack.com/client/${installation.team_id}`)
          }
          className="inline-flex items-center gap-1 text-caption text-muted-foreground underline-offset-2 transition-colors hover:text-foreground hover:underline"
          title={t(($) => $.slack.agent_bot_manage_tooltip)}
        >
          <ExternalLink className="h-3 w-3" />
          {t(($) => $.slack.agent_bot_manage_link)}
        </button>
      )}

      <AlertDialog
        open={confirmOpen}
        onOpenChange={(v) => {
          if (!v && !disconnecting) setConfirmOpen(false);
        }}
      >
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>
              {t(($) => $.slack.disconnect_confirm_title)}
            </AlertDialogTitle>
            <AlertDialogDescription>
              {t(($) => $.slack.disconnect_confirm_description)}
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel disabled={disconnecting}>
              {t(($) => $.slack.disconnect_confirm_cancel)}
            </AlertDialogCancel>
            <AlertDialogAction onClick={handleDisconnect} disabled={disconnecting}>
              {disconnecting
                ? t(($) => $.slack.disconnecting)
                : t(($) => $.slack.disconnect)}
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </div>
  );
}
