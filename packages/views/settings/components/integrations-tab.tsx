"use client";

import { useEffect, useRef, useState, type ReactNode } from "react";
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
import { isMessagingInstallationHealthy } from "@patchbay/core/types";
import { memberListOptions } from "@patchbay/core/workspace/queries";
import { larkInstallationsOptions, larkKeys } from "@patchbay/core/lark";
import { slackInstallationsOptions, slackKeys } from "@patchbay/core/slack";
import { dingtalkInstallationsOptions, dingtalkKeys } from "@patchbay/core/dingtalk";
import { wecomInstallationsOptions, wecomKeys } from "@patchbay/core/wecom";
import { telegramInstallationsOptions, telegramKeys } from "@patchbay/core/telegram";
import { weixinInstallationsOptions, weixinKeys } from "@patchbay/core/weixin";
import { composioToolkitsOptions } from "@patchbay/core/composio";
import { useT } from "../../i18n";
import { useNavigation } from "../../navigation";
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
import { IntegrationSetupGuide } from "./integration-setup-guide";

type InstallationSummary = {
  id: string;
  agent_id: string | null;
  status: string;
  runtime?: {
    state: string;
    observedAt: string | null;
    errorCode: string | null;
  };
  setup?: {
    experimental?: boolean;
  };
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
  isGuest: boolean;
  setupWritable: boolean;
  query: IntegrationQuery;
  installationId?: string;
  reconnectSupported?: boolean;
  onDisconnect: () => void;
  onManage: () => void;
  onReconnect: () => void;
};

function formatQuotaResetAt(value: string | null | undefined): string | null {
  if (!value) return null;
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) return null;
  return date.toLocaleDateString(undefined, {
    year: "numeric",
    month: "short",
    day: "numeric",
  });
}

function hasActiveHub(listing: InstallationListing | undefined) {
  return (
    listing?.installations.some(
      (installation) => installation.agent_id === null && installation.status === "active",
    ) ?? false
  );
}

function isHealthy(installation: InstallationSummary) {
  return isMessagingInstallationHealthy(installation);
}

function hasActiveInstallation(listing: InstallationListing | undefined) {
  // The durable installation must stay addressable for manage/reconnect/
  // disconnect actions even when its observed transport is offline.
  return listing?.installations.some((installation) => installation.status === "active") ?? false;
}

function hasHealthyAgentInstallation(listing: InstallationListing | undefined) {
  return (
    listing?.installations.some(
      (installation) => installation.agent_id !== null && isHealthy(installation),
    ) ?? false
  );
}

function hasHostedQuotaPause(listing: InstallationListing | undefined) {
  return (
    listing?.installations.some(
      (installation) =>
        installation.status === "active" &&
        installation.runtime?.errorCode === "hosted_quota_paused",
    ) ?? false
  );
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
  if (query.data.installations.some((installation) => installation.agent_id === null && isHealthy(installation))) {
    return (
      <Badge className="bg-emerald-600 text-white hover:bg-emerald-600">
        <CheckCircle2 />
        {t(($) => $.page.integrations_connected)}
      </Badge>
    );
  }
  if (hasHealthyAgentInstallation(query.data)) {
    return <Badge variant="outline">{t(($) => $.page.integrations_existing_agent)}</Badge>;
  }
  if (hasHostedQuotaPause(query.data)) {
    return (
      <Badge variant="outline">
        {t(($) => $.page.integrations_runtime_quota_paused)}
      </Badge>
    );
  }
  if (
    query.data.installations.some(
      (installation) =>
        installation.status === "active" && installation.setup?.experimental === true,
    )
  ) {
    return <Badge variant="outline">{t(($) => $.page.integrations_experimental)}</Badge>;
  }
  const activeUnhealthy = query.data.installations.find(
    (installation) => installation.status === "active" && !isHealthy(installation),
  );
  if (activeUnhealthy) {
    const runtime = activeUnhealthy.runtime;
    let label = t(($) => $.page.integrations_status);
    if (runtime?.state === "starting") {
      label = t(($) => $.page.integrations_runtime_starting);
    } else if (runtime?.errorCode === "configuration_invalid") {
      label = t(($) => $.page.integrations_runtime_configuration_error);
    } else if (runtime?.state === "offline") {
      label = t(($) => $.page.integrations_runtime_offline);
    } else if (runtime?.state === "degraded") {
      label = t(($) => $.page.integrations_runtime_degraded);
    } else if (runtime?.state === "error") {
      label = t(($) => $.page.integrations_runtime_error);
    }
    return <Badge variant="outline">{label}</Badge>;
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
  installationId,
  isGuest,
  setupWritable,
  onDisconnect,
  onManage,
  onReconnect,
  query,
  reconnectSupported = true,
}: HubActionProps) {
  const { t } = useT("settings");
  // Keep an active installation addressable even when its transport is
  // offline. This lets an admin reconnect or disconnect the durable server
  // record instead of creating a duplicate installation.
  const hubConnected = hasActiveHub(query.data);
  const installationConnected = hasActiveInstallation(query.data);

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

  if (!setupWritable) {
    return (
      <span className="text-caption text-muted-foreground">
        {t(($) => $.page.integrations_server_managed)}
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
  if (installationConnected && !hubConnected) {
    return (
      <Button variant="outline" size="sm" onClick={onManage}>
        <Settings2 />
        {t(($) => $.page.integrations_manage)}
      </Button>
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
  return (
    <Button variant="outline" size="sm" onClick={onManage}>
      {t(($) => $.page.integrations_start_setup)}
    </Button>
  );
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
  const navigation = useNavigation();
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
      (installation) =>
        installation.agent_id !== null && installation.status === "active",
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
  const messaging = useConfigStore((state) => state.messaging);
  // A missing capability contract is intentionally read-only. The server's
  // capability contract is the authority for whether this page may mutate an
  // installation; a missing field must never silently re-enable writes.
  const setupWritable = messaging?.setupWritable === true;
  const slackConnected = navigation.searchParams.get("slack_connected");
  const slackError = navigation.searchParams.get("slack_error");
  const consumedSlackCallback = useRef<string | null>(null);
  useEffect(() => {
    const callbackKey = slackConnected
      ? `connected:${slackConnected}`
      : slackError
        ? `error:${slackError}`
        : null;
    if (!callbackKey || consumedSlackCallback.current === callbackKey) return;
    consumedSlackCallback.current = callbackKey;
    if (slackConnected) {
      toast.success(t(($) => $.slack.connect_success_toast));
      void qc.invalidateQueries({ queryKey: slackKeys.installations(wsId) });
    } else if (slackError === "slack_authorization_denied") {
      toast.error(t(($) => $.slack.oauth_denied_toast));
    } else if (slackError === "im_installation_limit_reached") {
      toast.error(t(($) => $.slack.oauth_limit_toast));
    } else if (
      slackError === "slack_authorization_changed" ||
      slackError === "slack_code_missing"
    ) {
      toast.error(t(($) => $.slack.oauth_expired_toast));
    } else {
      toast.error(t(($) => $.slack.oauth_failed_toast));
    }
    const params = new URLSearchParams(navigation.searchParams);
    params.delete("slack_connected");
    params.delete("slack_error");
    const query = params.toString();
    navigation.replace(query ? `${navigation.pathname}?${query}` : navigation.pathname);
    // Callback parameters are one-shot provider state. The ref prevents the
    // Strict Mode double effect before navigation commits the replacement.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [slackConnected, slackError]);
  const messagingQuota = useQuery({
    queryKey: ["messaging-quota", wsId],
    queryFn: () => api.getMessagingQuotaUsage(wsId),
    enabled: !!wsId && messaging?.mode === "managed",
    staleTime: 30_000,
  });
  const quotaResetAt = formatQuotaResetAt(messagingQuota.data?.reset_at);
  const quotaConsumed =
    messagingQuota.data?.used !== null && messagingQuota.data?.used !== undefined
      ? messagingQuota.data.used + (messagingQuota.data.reserved ?? 0)
      : null;

  const listings = { lark, slack, dingtalk, wecom, telegram, weixin };
  const managedListing = managedChannel ? listings[managedChannel].data : undefined;
  const managedChannelNeedsSetup = Boolean(
    managedChannel && !hasActiveInstallation(managedListing),
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

  function renderSetupAction(channel: IntegrationChannel) {
    return {
      lark: <LarkAgentBindButton workspaceScoped />,
      slack: <SlackAgentBindButton />,
      dingtalk: <DingTalkAgentBindButton />,
      wecom: <WecomAgentBindButton />,
      telegram: <TelegramAgentBindButton />,
      weixin: <WeixinAgentBindButton />,
    }[channel];
  }

  function renderManagedContent(channel: IntegrationChannel) {
    const listing = listings[channel].data;
    if (hasActiveInstallation(listing)) return renderManagedTab(channel);

    return (
      <div className="space-y-5">
        <IntegrationSetupGuide channel={channel} managed={messaging?.mode === "managed"} />
        {listing?.configured && listing.install_supported ? (
          <div className="flex justify-end">{renderSetupAction(channel)}</div>
        ) : (
          <p className="text-caption text-destructive" role="alert">
            {t(($) => $.page.integrations_setup_unavailable)}
          </p>
        )}
      </div>
    );
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
          {messaging?.mode === "managed" ? (
            <div
              className="mt-3 flex flex-wrap items-center gap-x-2 gap-y-1 text-caption text-muted-foreground"
              data-testid="messaging-quota"
            >
              <span className="font-medium text-foreground">
                {t(($) => $.page.integrations_quota_title)}
              </span>
              {messagingQuota.isLoading ? (
                <span>{t(($) => $.page.integrations_quota_loading)}</span>
              ) : messagingQuota.isError || !messagingQuota.data ? (
                <span>{t(($) => $.page.integrations_quota_unavailable)}</span>
              ) : messagingQuota.data.mode === "unavailable" ? (
                <span>{t(($) => $.page.integrations_quota_unavailable)}</span>
              ) : messagingQuota.data.mode === "unlimited" && messagingQuota.data.limit === null ? (
                <span>{t(($) => $.page.integrations_quota_unlimited)}</span>
              ) : messagingQuota.data.mode === "managed" &&
                messagingQuota.data.limit !== null &&
                quotaConsumed !== null ? (
                <span>
                  {t(($) => $.page.integrations_quota_used, {
                    used: quotaConsumed,
                    limit: messagingQuota.data.limit,
                  })}
                  {quotaResetAt
                    ? ` · ${t(($) => $.page.integrations_quota_resets, { date: quotaResetAt })}`
                    : null}
                </span>
              ) : (
                <span>{t(($) => $.page.integrations_quota_unavailable)}</span>
              )}
            </div>
          ) : null}
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
                    (installation) => installation.agent_id === null && installation.status === "active",
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
                    setupWritable={setupWritable}
                    query={query}
                    installationId={hub?.id}
                    reconnectSupported={channel !== "lark" || larkHubRegion !== "lark"}
                    onManage={() => {
                      setManagedChannel(channel);
                      setManagedInstallationId(hub?.id ?? null);
                    }}
                    onReconnect={() => hub && setPendingAction({ channel, installationId: hub.id, reconnect: true })}
                    onDisconnect={() => hub && setPendingAction({ channel, installationId: hub.id, reconnect: false })}
                  />
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
          {managedChannel ? renderManagedContent(managedChannel) : null}
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
