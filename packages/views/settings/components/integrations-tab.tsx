"use client";

import { useState, type ReactNode } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { toast } from "sonner";
import { CheckCircle2, CircleAlert, Loader2, Settings2, Trash2 } from "lucide-react";
import { Card, CardContent } from "@patchbay/ui/components/ui/card";
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
  DialogHeader,
  DialogTitle,
} from "@patchbay/ui/components/ui/dialog";
import { ApiError } from "@patchbay/core/api";
import { api } from "@patchbay/core/api";
import { useConfigStore, useFeatureEnabled } from "@patchbay/core/config";
import { COMPOSIO_MCP_APPS_FLAG } from "@patchbay/core/feature-flags";
import { useAuthStore } from "@patchbay/core/auth";
import { useWorkspaceId } from "@patchbay/core/hooks";
import { memberListOptions } from "@patchbay/core/workspace/queries";
import { linearConnectionOptions } from "@patchbay/core/linear";
import { larkInstallationsOptions, larkKeys } from "@patchbay/core/lark";
import { slackInstallationsOptions, slackKeys } from "@patchbay/core/slack";
import { dingtalkInstallationsOptions, dingtalkKeys } from "@patchbay/core/dingtalk";
import { wecomInstallationsOptions, wecomKeys } from "@patchbay/core/wecom";
import { telegramInstallationsOptions, telegramKeys } from "@patchbay/core/telegram";
import { weixinInstallationsOptions, weixinKeys } from "@patchbay/core/weixin";
import { composioToolkitsOptions } from "@patchbay/core/composio";
import { useT } from "../../i18n";
import { SettingsSection, SettingsTab } from "./settings-layout";
import {
  IntegrationChannelIcon,
  type IntegrationChannel,
} from "./integration-channel-icon";
import { ComposioTab } from "./composio-tab";
import { VCSTab } from "./vcs-tab";
import { LarkAgentBindButton } from "./lark-tab";
import { SlackAgentBindButton } from "./slack-tab";
import { DingTalkAgentBindButton, DingTalkTab } from "./dingtalk-tab";
import { LarkTab } from "./lark-tab";
import { SlackTab } from "./slack-tab";
import { WecomAgentBindButton } from "./wecom-tab";
import { WecomTab } from "./wecom-tab";
import { TelegramAgentBindButton } from "./telegram-tab";
import { TelegramTab } from "./telegram-tab";
import { WeixinAgentBindButton } from "./weixin-tab";
import { WeixinTab } from "./weixin-tab";

type InstallationSummary = {
  id: string;
  agent_id: string | null;
  status: string;
};

type InstallationListing = {
  configured: boolean;
  install_supported?: boolean;
  installations: readonly InstallationSummary[];
};

type IntegrationQuery = {
  data?: InstallationListing;
  isError: boolean;
  isLoading: boolean;
};

type IntegrationCardProps = {
  action: ReactNode;
  channel: IntegrationChannel;
  description: string;
  iconClassName: string;
  status: ReactNode;
  title: string;
};

type HubActionProps = {
  canManage: boolean;
  children: ReactNode;
  isGuest: boolean;
  query: IntegrationQuery;
  installationId?: string;
  reconnectSupported?: boolean;
  onDisconnect: () => void;
  onManage: () => void;
  onReconnect: () => void;
};

function hasActiveHub(listing: InstallationListing | undefined) {
  return (
    listing?.installations.some(
      (installation) =>
        installation.agent_id === null && installation.status === "active",
    ) ?? false
  );
}

function hasActiveInstallation(listing: InstallationListing | undefined) {
  return listing?.installations.some((installation) => installation.status === "active") ?? false;
}

function ConnectionStatus({ query }: { query: IntegrationQuery }) {
  const { t } = useT("settings");
  if (query.isLoading) {
    return (
      <Badge variant="secondary">
        <Loader2 className="animate-spin" />
        {t(($) => $.page.integrations_loading)}
      </Badge>
    );
  }
  if (query.isError || !query.data) {
    return (
      <div className="flex flex-wrap items-center gap-2" role="alert">
        <Badge variant="destructive">
          <CircleAlert />
          {t(($) => $.page.integrations_unavailable)}
        </Badge>
        {query.isError ? (
          <span className="text-micro text-destructive">
            {t(($) => $.page.integrations_health_error)}
          </span>
        ) : null}
      </div>
    );
  }
  if (hasActiveHub(query.data)) {
    return (
      <Badge className="bg-emerald-600 text-white hover:bg-emerald-600">
        <CheckCircle2 />
        {t(($) => $.page.integrations_connected)}
      </Badge>
    );
  }
  if (hasActiveInstallation(query.data)) {
    return <Badge variant="outline">{t(($) => $.page.integrations_existing_agent)}</Badge>;
  }
  const nonActive = query.data.installations.find((installation) => installation.status !== "active");
  if (nonActive) {
    return (
      <Badge variant="outline">
        {nonActive.status === "revoked"
          ? t(($) => $.page.integrations_revoked)
          : t(($) => $.page.integrations_status)}
      </Badge>
    );
  }
  if (!query.data.configured) {
    return <Badge variant="outline">{t(($) => $.page.integrations_setup_required)}</Badge>;
  }
  if (!query.data.install_supported) {
    return <Badge variant="outline">{t(($) => $.page.integrations_coming_soon)}</Badge>;
  }
  return <Badge variant="outline">{t(($) => $.page.integrations_disconnected)}</Badge>;
}

function HubAction({
  canManage,
  children,
  installationId,
  isGuest,
  onDisconnect,
  onManage,
  onReconnect,
  query,
  reconnectSupported = true,
}: HubActionProps) {
  const { t } = useT("settings");
  const hubConnected = hasActiveHub(query.data);

  // A guest may own its temporary workspace, but external platform
  // authorization is still a formal-account-only operation. Keep this gate
  // ahead of the workspace role check so a guest owner cannot reach a real
  // provider dialog by virtue of owning the guest workspace.
  if (isGuest) {
    return (
      <span className="text-caption text-muted-foreground">
        {t(($) => $.page.integrations_login_required)}
      </span>
    );
  }

  if (!canManage) {
    return (
      <span className="text-caption text-muted-foreground">
        {hubConnected
          ? t(($) => $.page.integrations_connected)
          : t(($) => $.page.integrations_admin_only)}
      </span>
    );
  }
  if (query.isLoading) {
    return (
      <span className="inline-flex items-center gap-2 text-caption text-muted-foreground">
        <Loader2 className="size-3.5 animate-spin" />
        {t(($) => $.page.integrations_loading)}
      </span>
    );
  }
  if (query.isError || !query.data) {
    return (
      <span className="text-caption text-muted-foreground">
        {t(($) => $.page.integrations_unavailable)}
      </span>
    );
  }
  if (!query.data.configured) {
    return (
      <Button variant="outline" size="sm" onClick={onManage}>
        <Settings2 />
        {t(($) => $.page.integrations_configure)}
      </Button>
    );
  }
  if (!query.data.install_supported && !hubConnected) {
    return (
      <Button variant="outline" size="sm" onClick={onManage}>
        <Settings2 />
        {t(($) => $.page.integrations_configure)}
      </Button>
    );
  }
  if (hubConnected && installationId) {
    // A deployment-wide capability flag is not enough for region-aware
    // providers. Lark's international reconnect flow is intentionally
    // disabled until its end-to-end path is restored; hiding the action
    // prevents revoking a working install into an unsupported flow.
    const canReconnect =
      reconnectSupported && query.data.install_supported === true;
    return (
      <div className="flex flex-wrap items-center justify-end gap-2">
        <Button variant="outline" size="sm" onClick={onManage}>
          <Settings2 />
          {t(($) => $.page.integrations_manage)}
        </Button>
        {canReconnect ? (
          <Button variant="outline" size="sm" onClick={onReconnect}>
            {t(($) => $.page.integrations_reconnect)}
          </Button>
        ) : null}
        <Button
          variant="ghost"
          size="sm"
          className="text-destructive hover:text-destructive"
          onClick={onDisconnect}
        >
          <Trash2 />
          {t(($) => $.page.integrations_disconnect)}
        </Button>
      </div>
    );
  }
  return children;
}

function IntegrationCard({
  action,
  channel,
  description,
  iconClassName,
  status,
  title,
}: IntegrationCardProps) {
  return (
    <Card
      className="h-full border-surface-border/80 shadow-none transition-colors hover:border-surface-border"
      data-testid={`integration-channel-card-${channel}`}
    >
      <CardContent className="flex min-h-52 flex-col gap-5 p-5">
        <div className="flex items-start gap-4">
          <IntegrationChannelIcon
            channel={channel}
            size="lg"
            className={iconClassName}
          />
          <div className="min-w-0 flex-1">
            <h3 className="text-body font-semibold">{title}</h3>
            <p className="mt-1.5 text-caption leading-5 text-muted-foreground">
              {description}
            </p>
          </div>
        </div>
        <div className="flex items-center gap-2">{status}</div>
        <div className="mt-auto flex min-h-9 items-center justify-end">
          {action}
        </div>
      </CardContent>
    </Card>
  );
}

// Integrations is the workspace-level connection surface. IM providers are
// connected once as workspace Hubs; the active Agent is selected from the
// conversation with `/agents`. Agent-specific installation controls remain
// available only from legacy deep links and Agent detail pages.
export function IntegrationsTab({ standalone = false }: { standalone?: boolean } = {}) {
  const { t } = useT("settings");
  const wsId = useWorkspaceId();
  const qc = useQueryClient();
  const [managedChannel, setManagedChannel] = useState<IntegrationChannel | null>(null);
  const [managedInstallationId, setManagedInstallationId] = useState<string | null>(null);
  const [pendingAction, setPendingAction] = useState<{
    channel: IntegrationChannel;
    installationId: string;
    reconnect: boolean;
  } | null>(null);
  const [mutating, setMutating] = useState(false);
  const [linearConnecting, setLinearConnecting] = useState(false);
  const user = useAuthStore((state) => state.user);
  const { data: members = [] } = useQuery({
    ...memberListOptions(wsId),
    enabled: !!wsId,
  });
  const currentMember = members.find((member) => member.user_id === user?.id);
  const canManage =
    currentMember?.role === "owner" || currentMember?.role === "admin";
  const isGuest = Boolean(
    user &&
      "is_guest" in user &&
      (user as { is_guest?: boolean }).is_guest === true,
  );

  const lark = useQuery({
    ...larkInstallationsOptions(wsId),
    enabled: !!wsId,
  });
  const slack = useQuery({
    ...slackInstallationsOptions(wsId),
    enabled: !!wsId,
  });
  const dingtalk = useQuery({
    ...dingtalkInstallationsOptions(wsId),
    enabled: !!wsId,
  });
  const wecom = useQuery({
    ...wecomInstallationsOptions(wsId),
    enabled: !!wsId,
  });
  const telegram = useQuery({
    ...telegramInstallationsOptions(wsId),
    enabled: !!wsId,
  });
  const weixin = useQuery({
    ...weixinInstallationsOptions(wsId),
    enabled: !!wsId,
  });
  const hasLegacyDingTalkInstallation =
    dingtalk.data?.installations.some(
      (installation) => installation.agent_id !== null,
    ) ?? false;

  const composioEnabled = useFeatureEnabled(COMPOSIO_MCP_APPS_FLAG, false);
  const composioToolkits = useQuery({
    ...composioToolkitsOptions(),
    enabled: composioEnabled,
  });
  const composioUnconfigured =
    composioToolkits.error instanceof ApiError &&
    composioToolkits.error.status === 503;
  const vcsAvailable = useConfigStore((state) => state.vcsIntegrationAvailable);
  const linear = useQuery({
    ...linearConnectionOptions(wsId),
    enabled: !!wsId,
  });

  async function connectLinear() {
    if (linearConnecting || isGuest || !canManage || !wsId) return;
    setLinearConnecting(true);
    try {
      const response = await api.startLinearOAuth(wsId);
      window.location.assign(response.authorization_url);
    } catch {
      toast.error(t(($) => $.page.linear.connect_failed));
      setLinearConnecting(false);
    }
  }

  const listings = { lark, slack, dingtalk, wecom, telegram, weixin };
  const managedListing = managedChannel ? listings[managedChannel].data : undefined;
  const managedChannelNeedsSetup = Boolean(
    managedChannel &&
      !hasActiveHub(managedListing) &&
      (!managedListing?.configured || !managedListing.install_supported),
  );

  async function removeInstallation(channel: IntegrationChannel, installationId: string) {
    if (mutating) return;
    setMutating(true);
    try {
      const remove = {
        lark: () => api.deleteLarkInstallation(wsId, installationId),
        slack: () => api.deleteSlackInstallation(wsId, installationId),
        dingtalk: () => api.deleteDingTalkInstallation(wsId, installationId),
        wecom: () => api.deleteWecomInstallation(wsId, installationId),
        telegram: () => api.deleteTelegramInstallation(wsId, installationId),
        weixin: () => api.deleteWeixinInstallation(wsId, installationId),
      }[channel];
      await remove();
      const key = {
        lark: larkKeys.installations(wsId),
        slack: slackKeys.installations(wsId),
        dingtalk: dingtalkKeys.installations(wsId),
        wecom: wecomKeys.installations(wsId),
        telegram: telegramKeys.installations(wsId),
        weixin: weixinKeys.installations(wsId),
      }[channel];
      await qc.invalidateQueries({ queryKey: key });
      toast.success(
        pendingAction?.reconnect
          ? t(($) => $.page.integrations_reconnect_ready)
          : t(($) => $.page.integrations_disconnected_toast),
      );
    } catch {
      toast.error(t(($) => $.page.integrations_action_failed));
    } finally {
      setMutating(false);
      setPendingAction(null);
    }
  }

  function renderManagedTab(channel: IntegrationChannel) {
    return {
      lark: <LarkTab installationId={managedInstallationId ?? undefined} />,
      slack: <SlackTab installationId={managedInstallationId ?? undefined} />,
      dingtalk: <DingTalkTab installationId={managedInstallationId ?? undefined} />,
      wecom: <WecomTab installationId={managedInstallationId ?? undefined} />,
      telegram: <TelegramTab installationId={managedInstallationId ?? undefined} />,
      weixin: <WeixinTab installationId={managedInstallationId ?? undefined} />,
    }[channel];
  }

  const content = (
    <>
      <section className="space-y-4">
        <div>
          <h3 className="text-body font-semibold">
            {t(($) => $.page.integrations_channels_title)}
          </h3>
          <p className="mt-1 max-w-3xl text-caption leading-5 text-muted-foreground">
            {t(($) => $.page.integrations_channels_description)}
          </p>
          <p className="mt-2 max-w-3xl text-caption leading-5 text-muted-foreground">
            {t(($) => $.page.integrations_route_note)}
          </p>
        </div>
        <div className="grid gap-4 md:grid-cols-2 xl:grid-cols-3">
          {([
            ["lark", t(($) => $.lark.section_title), t(($) => $.lark.workspace_description), "bg-[#3370FF]/10"],
            ["slack", t(($) => $.slack.section_title), t(($) => $.slack.workspace_description), "bg-[#611f69]/10"],
            ["dingtalk", t(($) => $.dingtalk.section_title), t(($) => $.dingtalk.workspace_description), "bg-[#1677FF]/10"],
            ["wecom", t(($) => $.wecom.section_title), t(($) => $.wecom.workspace_description), "bg-[#07C160]/10"],
            ["telegram", t(($) => $.telegram.section_title), t(($) => $.telegram.workspace_description), "bg-[#2AABEE]/10"],
            ["weixin", t(($) => $.weixin.section_title), t(($) => $.weixin.workspace_description), "bg-[#07C160]/10"],
          ] as const).map(([channel, title, description, iconClassName]) => {
            const query = listings[channel];
            const hub = query.data?.installations.find(
              (installation) => installation.agent_id === null && installation.status === "active",
            );
            const larkHubRegion =
              channel === "lark"
                ? lark.data?.installations.find(
                    (installation) =>
                      installation.agent_id === null && installation.status === "active",
                  )?.region
                : undefined;
            return (
              <IntegrationCard
                key={channel}
                channel={channel}
                title={title}
                description={description}
                iconClassName={iconClassName}
                status={<ConnectionStatus query={query} />}
                action={
                  <HubAction
                    canManage={canManage}
                    isGuest={isGuest}
                    query={query}
                    installationId={hub?.id}
                    reconnectSupported={channel !== "lark" || larkHubRegion !== "lark"}
                    onManage={() => {
                      setManagedChannel(channel);
                      setManagedInstallationId(hub?.id ?? null);
                    }}
                    onReconnect={() => hub && setPendingAction({ channel, installationId: hub.id, reconnect: true })}
                    onDisconnect={() => hub && setPendingAction({ channel, installationId: hub.id, reconnect: false })}
                  >
                    {channel === "lark" ? <LarkAgentBindButton workspaceScoped /> : null}
                    {channel === "slack" ? <SlackAgentBindButton /> : null}
                    {channel === "dingtalk" ? <DingTalkAgentBindButton /> : null}
                    {channel === "wecom" ? <WecomAgentBindButton /> : null}
                    {channel === "telegram" ? <TelegramAgentBindButton /> : null}
                    {channel === "weixin" ? <WeixinAgentBindButton /> : null}
                  </HubAction>
                }
              />
            );
          })}
        </div>
      </section>

      {hasLegacyDingTalkInstallation ? (
        <SettingsSection
          title={t(($) => $.dingtalk.legacy_management_title)}
          description={t(($) => $.dingtalk.legacy_management_description)}
        >
          <DingTalkTab />
        </SettingsSection>
      ) : null}

      {composioEnabled && !composioUnconfigured ? (
        <SettingsSection title={t(($) => $.composio.section_title)}>
          <ComposioTab />
        </SettingsSection>
      ) : null}
      <SettingsSection
        title={t(($) => $.page.linear.section_title)}
        description={t(($) => $.page.linear.description)}
      >
        <Card className="border-surface-border/80 shadow-none" data-testid="linear-integration-card">
          <CardContent className="flex flex-col gap-4 p-5 sm:flex-row sm:items-center sm:justify-between">
            <div className="min-w-0 space-y-2">
              <div className="flex flex-wrap items-center gap-2">
                <h3 className="text-body font-semibold">Linear</h3>
                {linear.isLoading ? (
                  <Badge variant="secondary">
                    <Loader2 className="animate-spin" />
                    {t(($) => $.page.linear.loading)}
                  </Badge>
                ) : linear.data?.connected ? (
                  <Badge className="bg-emerald-600 text-white hover:bg-emerald-600">
                    <CheckCircle2 />
                    {t(($) => $.page.linear.connected, {
                      organization:
                        linear.data.connection?.organization_name ??
                        linear.data.connection?.organization_id ??
                        "Linear",
                    })}
                  </Badge>
                ) : (
                  <Badge variant="outline">{t(($) => $.page.linear.not_connected)}</Badge>
                )}
              </div>
              {linear.data?.connected ? (
                <p className="text-caption text-muted-foreground">
                  {t(($) => $.page.linear.project_bindings, {
                    count: linear.data.project_bindings.length,
                  })}
                </p>
              ) : null}
              {linear.isError ? (
                <p className="text-caption text-destructive" role="alert">
                  {t(($) => $.page.linear.connect_failed)}
                </p>
              ) : null}
            </div>
            <div className="shrink-0">
              {isGuest ? (
                <span className="text-caption text-muted-foreground">
                  {t(($) => $.page.linear.login_required)}
                </span>
              ) : canManage ? (
                <Button
                  variant="outline"
                  size="sm"
                  disabled={linearConnecting}
                  onClick={() => void connectLinear()}
                >
                  {linearConnecting ? <Loader2 className="animate-spin" /> : null}
                  {t(($) =>
                    linearConnecting
                      ? $.page.linear.connecting
                      : $.page.linear.connect,
                  )}
                </Button>
              ) : (
                <span className="text-caption text-muted-foreground">
                  {t(($) => $.page.linear.admin_only)}
                </span>
              )}
            </div>
          </CardContent>
        </Card>
      </SettingsSection>
      {vcsAvailable ? (
        <SettingsSection title={t(($) => $.vcs.section_title)}>
          <VCSTab />
        </SettingsSection>
      ) : null}
    </>
  );

  return (
    <>
      {standalone ? (
        <div className="mx-auto w-full max-w-6xl space-y-8 p-4 sm:p-6 lg:p-8">
          <SettingsTab title={t(($) => $.page.integrations_title)} description={t(($) => $.page.integrations_description)}>
            {content}
          </SettingsTab>
        </div>
      ) : (
        <SettingsTab title={t(($) => $.page.tabs.integrations)} description={t(($) => $.page.integrations_description)}>
          {content}
        </SettingsTab>
      )}
      <Dialog
        open={managedChannel !== null}
        onOpenChange={(open) => {
          if (!open) {
            setManagedChannel(null);
            setManagedInstallationId(null);
          }
        }}
      >
        <DialogContent className="max-h-[90vh] overflow-y-auto sm:max-w-3xl">
          <DialogHeader>
            <DialogTitle>
              {managedChannelNeedsSetup
                ? t(($) => $.page.integrations_setup_title)
                : t(($) => $.page.integrations_manage)}
            </DialogTitle>
          </DialogHeader>
          {managedChannel ? renderManagedTab(managedChannel) : null}
        </DialogContent>
      </Dialog>
      <AlertDialog open={pendingAction !== null} onOpenChange={(open) => !open && !mutating && setPendingAction(null)}>
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>
              {pendingAction?.reconnect
                ? t(($) => $.page.integrations_reconnect_title)
                : t(($) => $.page.integrations_disconnect_title)}
            </AlertDialogTitle>
            <AlertDialogDescription>
              {pendingAction?.reconnect
                ? t(($) => $.page.integrations_reconnect_description)
                : t(($) => $.page.integrations_disconnect_description)}
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel disabled={mutating}>{t(($) => $.page.integrations_cancel)}</AlertDialogCancel>
            <AlertDialogAction
              disabled={mutating || !pendingAction}
              onClick={() => {
                if (pendingAction) void removeInstallation(pendingAction.channel, pendingAction.installationId);
              }}
            >
              {pendingAction?.reconnect
                ? t(($) => $.page.integrations_reconnect_confirm)
                : t(($) => $.page.integrations_disconnect)}
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </>
  );
}
