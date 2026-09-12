"use client";

import { MessagingConnectionStatus } from "./messaging-connection-status";
import { MessagingSetupNotice, useMessagingSetupWritable } from "./messaging-setup-policy";

import { useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { toast } from "sonner";
import { ChevronRight, ExternalLink, Trash2 } from "lucide-react";
// Two design systems in one file, on purpose, and the split is by *surface*
// rather than by component: `TelegramTab` lives only inside the settings
// dialog, which mounts `LobeThemeBridge`, so it is Lobe.
// `TelegramAgentBindButton` and its two sub-components are rendered from the
// **agent detail page**
// (`packages/views/agents/components/tabs/integrations-tab.tsx`), and that
// surface has no bridge — `packages/views/agents/**` contains no
// `@lobehub/ui` import at all. Every Lobe primitive they would need (`Button`,
// `Modal`) calls `useMotionComponent()` and throws `Please wrap your app with
// <ConfigProvider> (or <MotionProvider>)` without one — measured, not assumed.
// So they keep the shadcn primitives until their own surface gets a bridge; the
// alias below is what keeps the two apart at the call sites. The Slack tab's
// header carries the long form of this note.
import { Button as LobeButton } from "@lobehub/ui/base-ui";
import { TelegramMark } from "./telegram-mark";
import { cn } from "@orvilo/ui/lib/utils";
import { Button } from "@orvilo/ui/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogHeader,
  DialogTitle,
} from "@orvilo/ui/components/ui/dialog";
import { CredentialFieldForm } from "./credential-field-form";
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@orvilo/ui/components/ui/alert-dialog";
import { SettingsEmptyState } from "./settings-empty";
import { useSettingsConfirm } from "./settings-confirm";
import { SettingsFormRow, SettingsGroup } from "./settings-shell";
import { useAuthStore } from "@orvilo/core/auth";
import { useWorkspaceId } from "@orvilo/core/hooks";
import { memberListOptions } from "@orvilo/core/workspace/queries";
import { useActorName } from "@orvilo/core/workspace/hooks";
import { telegramInstallationsOptions, telegramKeys } from "@orvilo/core/telegram";
import { api } from "@orvilo/core/api";
import type { TelegramInstallation } from "@orvilo/core/types";
import { ActorAvatar } from "../../common/actor-avatar";
import { openExternal } from "../../platform";
import { useLocale, useT } from "../../i18n";

// TelegramTab is the workspace settings panel for Telegram bot installations,
// mirroring SlackTab: listing is member-visible; the disconnect action is
// admin-only (backend-enforced; the UI hides the button to match). Adding a
// new installation flows through the Agent detail page — the install path is
// per-agent (one bot per agent, the (workspace_id, agent_id, channel_type)
// UNIQUE in channel_installation).
export function TelegramTab() {
  const setupWritable = useMessagingSetupWritable();
  const { t } = useT("settings");
  const wsId = useWorkspaceId();
  const qc = useQueryClient();
  const user = useAuthStore((s) => s.user);

  const { data: members = [] } = useQuery(memberListOptions(wsId));
  const currentMember = members.find((m) => m.user_id === user?.id) ?? null;
  const canManage =
    currentMember?.role === "owner" || currentMember?.role === "admin";

  const { data, isLoading, isError } = useQuery({
    ...telegramInstallationsOptions(wsId),
    enabled: !!wsId,
  });
  const installations = data?.installations ?? [];
  const configured = data?.configured === true;

  const confirm = useSettingsConfirm();

  /**
   * Await the server before touching cache/UI (repo rule: no optimistic removal
   * on flows that confirm/destroy), and reject on failure on purpose:
   * `useSettingsConfirm`'s `onOk` keeps the dialog open only while its promise
   * is unsettled, so swallowing the error here would dismiss the confirmation
   * exactly when the bot is still installed.
   */
  async function handleDisconnect(installationId: string) {
    if (!setupWritable) return;
    try {
      await api.deleteTelegramInstallation(wsId, installationId);
      await qc.invalidateQueries({ queryKey: telegramKeys.installations(wsId) });
      toast.success(t(($) => $.telegram.toast_disconnected));
    } catch (e) {
      toast.error(
        e instanceof Error ? e.message : t(($) => $.telegram.toast_disconnect_failed),
      );
      throw e;
    }
  }

  const openDisconnectConfirm = (installationId: string) =>
    confirm({
      title: t(($) => $.telegram.disconnect_confirm_title),
      description: t(($) => $.telegram.disconnect_confirm_description),
      confirmLabel: t(($) => $.telegram.disconnect),
      cancelLabel: t(($) => $.telegram.disconnect_confirm_cancel),
      onConfirm: () => handleDisconnect(installationId),
    });

  return (
    <div className="space-y-8">
      {!setupWritable && <MessagingSetupNotice />}
      {/* The loading and load-failure cards were two branches of "there is
          nothing to list"; both are the same empty state, and `SettingsEmptyState`
          is where an empty state lives. */}
      {isError ? (
        <SettingsGroup variant="outlined" title={t(($) => $.telegram.installed_bots)}>
          <SettingsEmptyState title={t(($) => $.telegram.load_failed)} />
        </SettingsGroup>
      ) : isLoading ? (
        <SettingsGroup variant="outlined" title={t(($) => $.telegram.installed_bots)}>
          <SettingsEmptyState title={t(($) => $.telegram.loading)} />
        </SettingsGroup>
      ) : !configured && installations.length === 0 ? (
        // A notice, not a settings group: there are no rows behind it, so the
        // paragraph is the group's *body*. `description` would render it in the
        // header, beside the title, where it reads as a subtitle.
        <SettingsGroup
          variant="outlined"
          title={t(($) => $.telegram.not_enabled_title)}
        >
          <p className="text-body text-muted-foreground">
            {t(($) => $.telegram.not_enabled_description_prefix)}{" "}
            <code className="rounded bg-muted px-1 py-0.5 text-micro">
              ORVILO_TELEGRAM_SECRET_KEY
            </code>{" "}
            {t(($) => $.telegram.not_enabled_description_suffix)}{" "}
            {t(($) => $.telegram.not_enabled_self_host_hint)}
          </p>
        </SettingsGroup>
      ) : (
        <SettingsGroup
          variant="outlined"
          title={t(($) => $.telegram.installed_bots)}
        >
          {installations.length === 0 ? (
            <SettingsEmptyState
              title={t(($) => $.telegram.empty_title)}
              description={
                <>
                  {t(($) => $.telegram.empty_description_prefix)}{" "}
                  <strong>{t(($) => $.telegram.empty_description_cta)}</strong>{" "}
                  {t(($) => $.telegram.empty_description_suffix)}
                </>
              }
            />
          ) : (
            installations.map((inst, index) => (
              <InstallationRow
                key={inst.id}
                divider={index > 0}
                installation={inst}
                canManage={canManage && setupWritable}
                onDisconnect={() => openDisconnectConfirm(inst.id)}
              />
            ))
          )}
        </SettingsGroup>
      )}
    </div>
  );
}

/** One installation, as a `SettingsFormRow` — see the Slack tab's twin. */
function InstallationRow({
  installation,
  canManage,
  divider,
  onDisconnect,
}: {
  installation: TelegramInstallation;
  canManage: boolean;
  divider: boolean;
  onDisconnect: () => void;
}) {
  const { t } = useT("settings");
  const locale = useLocale();
  const { getAgentName } = useActorName();
  const isInstalled = installation.status === "installed";
  const agentName = installation.agent_id
    ? getAgentName(installation.agent_id)
    : t(($) => $.page.integrations_workspace_connection);
  return (
    <SettingsFormRow
      divider={divider}
      label={
        <span className="inline-flex min-w-0 items-center gap-2">
          {installation.agent_id ? (
            <ActorAvatar
              actorType="agent"
              actorId={installation.agent_id}
              size="lg"
              enableHoverCard
              profileLink
            />
          ) : null}
          <span className="min-w-0 truncate">{agentName}</span>
          {installation.bot_username ? (
            <span className="shrink-0 text-caption text-muted-foreground">
              @{installation.bot_username}
            </span>
          ) : null}
          {!isInstalled ? (
            <span className="shrink-0 rounded bg-muted px-1.5 py-0.5 text-micro text-muted-foreground">
              {t(($) => $.telegram.revoked_badge)}
            </span>
          ) : null}
        </span>
      }
      description={
        <>
          {/* Not `compact`: this row has always rendered the full status
              (label + badge). The full branch is the one that reports an
              `error` install as the destructive/red pill — the compact branch
              has two dot colours, so `error` would read amber and look exactly
              like `disconnected`. See the Slack tab's row for the long form. */}
          <MessagingConnectionStatus installation={installation} />
          <span className="block">
            {t(($) => $.telegram.installed_at_label, {
              when: new Date(installation.installed_at).toLocaleString(locale),
            })}
          </span>
        </>
      }
    >
      {canManage && isInstalled ? (
        <LobeButton onClick={onDisconnect}>
          <Trash2 className="h-3 w-3" />
          {t(($) => $.telegram.disconnect)}
        </LobeButton>
      ) : null}
    </SettingsFormRow>
  );
}

// telegramDocsUrl points at the Telegram integration guide on the docs site,
// localized like the Slack docs link.
function telegramDocsUrl(lang: string | undefined): string {
  const prefix = lang?.startsWith("zh") ? "/zh" : "";
  return `https://orvilo.aspectlylabs.com/docs${prefix}/telegram-bot-integration`;
}

// TelegramAgentBindButton is the per-agent CTA on the agent detail page.
// Telegram uses the paste-a-token model: the admin creates a bot with
// @BotFather and pastes its token; the backend validates via getMe before
// persisting. Visibility mirrors SlackAgentBindButton (owner/admin only).
export function TelegramAgentBindButton({
  agentId,
  agentName,
  className,
  onShowConnectedDetails,
}: {
  agentId?: string;
  agentName?: string;
  className?: string;
  /** Compact read-only connected row that invokes this instead of the full
   * badge — the agent inspector passes a "jump to the Integrations tab"
   * handler so management actions live in one place. */
  onShowConnectedDetails?: () => void;
}) {
  const { t, i18n } = useT("settings");
  const wsId = useWorkspaceId();
  const qc = useQueryClient();
  const user = useAuthStore((s) => s.user);

  const [dialogOpen, setDialogOpen] = useState(false);
  const [botToken, setBotToken] = useState("");
  const [submitting, setSubmitting] = useState(false);

  const { data: listing, isError: installationQueryFailed } = useQuery({
    ...telegramInstallationsOptions(wsId),
    enabled: !!wsId,
  });
  const installSupported = listing?.install_supported === true;
  const setupWritable = useMessagingSetupWritable();

  const { data: members = [] } = useQuery({
    ...memberListOptions(wsId),
    enabled: !!wsId,
  });
  const currentMember = members.find((m) => m.user_id === user?.id) ?? null;
  const canManage =
    currentMember?.role === "owner" || currentMember?.role === "admin";

  if (!canManage || user?.is_guest === true) return null;

  const recordedInstallation = listing?.installations.find(
    (inst) =>
      (agentId ? inst.agent_id === agentId : !inst.agent_id) &&
      inst.status === "installed",
  );
  const existing = recordedInstallation && installationQueryFailed
    ? { ...recordedInstallation, runtime: undefined }
    : recordedInstallation;
  if (existing) {
    return onShowConnectedDetails ? (
      <TelegramAgentBotStatusRow
        installation={existing}
        onClick={onShowConnectedDetails}
        className={className}
      />
    ) : (
      <TelegramAgentBotInstalledControls installation={existing} className={className} />
    );
  }

  if (!setupWritable) return <MessagingSetupNotice />;
  if (!installSupported) return null;

  function closeDialog() {
    if (submitting) return;
    setDialogOpen(false);
    setBotToken("");
  }

  async function handleSubmit() {
    const bot_token = botToken.trim();
    if (submitting || !wsId || !bot_token) return;
    setSubmitting(true);
    try {
      const installation = await api.registerTelegramBot(wsId, agentId, { bot_token });
      if (!installation.id || installation.status !== "installed") {
        throw new Error("Telegram returned an invalid installation response");
      }
      // The telegram_installation realtime event also refreshes this list, but
      // invalidate explicitly so the installed controls appear immediately.
      await qc.invalidateQueries({ queryKey: telegramKeys.installations(wsId) });
      toast.success(t(($) => $.telegram.connect_success_toast));
      setDialogOpen(false);
      setBotToken("");
    } catch (e) {
      toast.error(
        e instanceof Error ? e.message : t(($) => $.telegram.connect_failed_toast),
      );
    } finally {
      setSubmitting(false);
    }
  }

  const canSubmit = botToken.trim() !== "" && !submitting;

  return (
    <div
      className={cn("flex flex-wrap items-center gap-2", className)}
      data-testid="telegram-agent-bind-buttons"
    >
      <Button
        onClick={() => setDialogOpen(true)}
        disabled={!wsId}
        title={
          agentName
            ? t(($) => $.telegram.bind_button_title, { agent: agentName })
            : undefined
        }
        data-testid="telegram-agent-connect"
      >
        <TelegramMark className="h-3 w-3" />
        {t(($) => $.telegram.bind_button)}
      </Button>

      {/* shadcn `Dialog`, not Lobe `Modal`: this component renders on the agent
          detail page, which has no `LobeThemeBridge`. See the import note. */}
      <Dialog
        open={dialogOpen}
        onOpenChange={(v) => (v ? setDialogOpen(true) : closeDialog())}
      >
        <DialogContent className="sm:max-w-lg" data-testid="telegram-connect-dialog">
          <DialogHeader>
            <DialogTitle>{t(($) => $.telegram.connect_dialog_title)}</DialogTitle>
          </DialogHeader>
          <CredentialFieldForm
            leading={
              <>
                <p className="text-body text-muted-foreground">
                  {t(($) => $.telegram.connect_dialog_description)}
                </p>
                <button
                  type="button"
                  onClick={() => openExternal(telegramDocsUrl(i18n.language))}
                  className="inline-flex w-fit items-center gap-2 text-body font-medium text-primary underline-offset-2 hover:underline"
                  data-testid="telegram-docs-link"
                >
                  <ExternalLink data-icon="inline-start" />
                  {t(($) => $.telegram.connect_docs_link)}
                </button>
              </>
            }
            fields={[
              {
                id: "telegram-bot-token",
                label: t(($) => $.telegram.bot_token_label),
                value: botToken,
                onChange: setBotToken,
                type: "password",
                placeholder: "123456789:AA…",
                testId: "telegram-bot-token",
                disabled: submitting,
              },
            ]}
            cancelLabel={t(($) => $.telegram.connect_cancel)}
            submitLabel={
              submitting
                ? t(($) => $.telegram.connect_submitting)
                : t(($) => $.telegram.connect_submit)
            }
            submitting={submitting}
            canSubmit={canSubmit}
            onCancel={closeDialog}
            onSubmit={() => void handleSubmit()}
            submitTestId="telegram-connect-submit"
          />
        </DialogContent>
      </Dialog>
    </div>
  );
}

// TelegramAgentBotStatusRow is the compact, read-only installation affordance the
// agent inspector renders; it deep-links into the Integrations tab.
function TelegramAgentBotStatusRow({
  installation,
  onClick,
  className,
}: {
  installation: TelegramInstallation;
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
      data-testid="telegram-agent-bot-status"
    >
      <span className="truncate">{t(($) => $.telegram.section_title)}</span>
      <MessagingConnectionStatus installation={installation} compact />
      <ChevronRight className="ml-auto h-3.5 w-3.5 shrink-0" />
    </button>
  );
}

// TelegramAgentBotInstalledControls is the full installed-bot affordance:
// status + Disconnect, then an "Open in Telegram" deep link to the bot.
function TelegramAgentBotInstalledControls({
  installation,
  className,
}: {
  installation: TelegramInstallation;
  className?: string;
}) {
  const { t } = useT("settings");
  const wsId = useWorkspaceId();
  const qc = useQueryClient();

  const [confirmOpen, setConfirmOpen] = useState(false);
  const setupWritable = useMessagingSetupWritable();
  const [disconnecting, setDisconnecting] = useState(false);

  async function handleDisconnect() {
    if (!setupWritable || disconnecting) return;
    setDisconnecting(true);
    try {
      await api.deleteTelegramInstallation(wsId, installation.id);
      await qc.invalidateQueries({ queryKey: telegramKeys.installations(wsId) });
      toast.success(t(($) => $.telegram.toast_disconnected));
      setConfirmOpen(false);
    } catch (e) {
      toast.error(
        e instanceof Error ? e.message : t(($) => $.telegram.toast_disconnect_failed),
      );
    } finally {
      setDisconnecting(false);
    }
  }

  return (
    <div
      className={cn("space-y-2", className)}
      data-testid="telegram-agent-bot-installed"
    >
      <div className="flex items-center justify-between gap-3">
        <span className="inline-flex min-w-0 items-center gap-2 text-caption text-muted-foreground">
          <span className="truncate">
            <MessagingConnectionStatus installation={installation} compact />
            {installation.bot_username ? ` · @${installation.bot_username}` : ""}
          </span>
        </span>
        {setupWritable && <Button
          variant="destructive"
          size="sm"
          onClick={() => setConfirmOpen(true)}
          disabled={disconnecting}
          title={t(($) => $.telegram.agent_bot_disconnect_tooltip)}
          aria-label={t(($) => $.telegram.disconnect)}
          data-testid="telegram-agent-bot-disconnect"
        >
          <Trash2 className="h-3 w-3" />
          {disconnecting
            ? t(($) => $.telegram.disconnecting)
            : t(($) => $.telegram.disconnect)}
        </Button>}
      </div>

      {installation.bot_username && (
        <button
          type="button"
          onClick={() => openExternal(`https://t.me/${installation.bot_username}`)}
          className="inline-flex items-center gap-1 text-caption text-muted-foreground underline-offset-2 transition-colors hover:text-foreground hover:underline"
          title={t(($) => $.telegram.agent_bot_manage_tooltip)}
        >
          <ExternalLink className="h-3 w-3" />
          {t(($) => $.telegram.agent_bot_manage_link)}
        </button>
      )}

      <AlertDialog
        open={setupWritable && confirmOpen}
        onOpenChange={(v) => {
          if (!v && !disconnecting) setConfirmOpen(false);
        }}
      >
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>
              {t(($) => $.telegram.disconnect_confirm_title)}
            </AlertDialogTitle>
            <AlertDialogDescription>
              {t(($) => $.telegram.disconnect_confirm_description)}
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel disabled={disconnecting}>
              {t(($) => $.telegram.disconnect_confirm_cancel)}
            </AlertDialogCancel>
            <AlertDialogAction onClick={handleDisconnect} disabled={disconnecting}>
              {disconnecting
                ? t(($) => $.telegram.disconnecting)
                : t(($) => $.telegram.disconnect)}
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>

    </div>
  );
}
