"use client";

import { useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { CircleAlert, Loader2, Settings2 } from "lucide-react";
import { ApiError, api } from "@patchbay/core/api";
import { useAuthStore } from "@patchbay/core/auth";
import { composioToolkitsOptions } from "@patchbay/core/composio";
import { useConfigStore, useFeatureEnabled } from "@patchbay/core/config";
import { dingtalkInstallationsOptions } from "@patchbay/core/dingtalk";
import {
  COMPOSIO_MCP_APPS_FLAG,
  LINEAR_INSTALLATION_FOUNDATION_FLAG,
} from "@patchbay/core/feature-flags";
import { larkInstallationsOptions } from "@patchbay/core/lark";
import { useCurrentWorkspace } from "@patchbay/core/paths";
import { slackInstallationsOptions } from "@patchbay/core/slack";
import { telegramInstallationsOptions } from "@patchbay/core/telegram";
import type { MessagingConnectionSource } from "@patchbay/core/types";
import { wecomInstallationsOptions } from "@patchbay/core/wecom";
import { weixinInstallationsOptions } from "@patchbay/core/weixin";
import { memberListOptions } from "@patchbay/core/workspace/queries";
import { Badge } from "@patchbay/ui/components/ui/badge";
import { Button } from "@patchbay/ui/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogHeader,
  DialogTitle,
} from "@patchbay/ui/components/ui/dialog";
import { useT } from "../../i18n";
import { ComposioTab } from "./composio-tab";
import { DingTalkAgentBindButton, DingTalkTab } from "./dingtalk-tab";
import { IntegrationCard } from "./integration-card";
import type { IntegrationChannel } from "./integration-channel-icon";
import { IntegrationSetupGuide } from "./integration-setup-guide";
import { LarkAgentBindButton, LarkTab } from "./lark-tab";
import { LinearIntegrationCard } from "./linear-tab";
import { MessagingConnectionStatus } from "./messaging-connection-status";
import { SettingsSection, SettingsTab } from "./settings-layout";
import { SlackAgentBindButton, SlackTab } from "./slack-tab";
import { TelegramAgentBindButton, TelegramTab } from "./telegram-tab";
import { VCSTab } from "./vcs-tab";
import { WecomAgentBindButton, WecomTab } from "./wecom-tab";
import { WeixinAgentBindButton, WeixinTab } from "./weixin-tab";

type MessagingChannel = Exclude<IntegrationChannel, "linear">;

type InstallationSummary = MessagingConnectionSource & {
  id: string;
  agent_id: string | null;
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

function isWorkspaceInstallation(agentId: string | null | undefined): boolean {
  return agentId == null || agentId === "";
}

function installedWorkspaceHub(listing: InstallationListing | undefined) {
  return listing?.installations.find(
    (installation) =>
      isWorkspaceInstallation(installation.agent_id) &&
      installation.status === "installed",
  );
}

function installedRecord(listing: InstallationListing | undefined) {
  return listing?.installations.find(
    (installation) => installation.status === "installed",
  );
}

function ChannelStatus({ query }: { query: IntegrationQuery }) {
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
      <Badge variant="destructive">
        <CircleAlert />
        {t(($) => $.page.integrations_unavailable)}
      </Badge>
    );
  }
  const hub = installedWorkspaceHub(query.data);
  if (hub) return <MessagingConnectionStatus installation={hub} compact />;
  if (installedRecord(query.data)) {
    return <Badge variant="outline">{t(($) => $.page.integrations_existing_agent)}</Badge>;
  }
  if (!query.data.configured) {
    return <Badge variant="outline">{t(($) => $.page.integrations_setup_required)}</Badge>;
  }
  return <Badge variant="outline">{t(($) => $.page.integrations_disconnected)}</Badge>;
}

function ChannelAction({
  canManage,
  isGuest,
  onOpen,
  query,
  setupWritable,
}: {
  canManage: boolean;
  isGuest: boolean;
  onOpen: () => void;
  query: IntegrationQuery;
  setupWritable: boolean;
}) {
  const { t } = useT("settings");
  if (isGuest) {
    return <span className="text-caption text-muted-foreground">{t(($) => $.page.integrations_login_required)}</span>;
  }
  if (!setupWritable) {
    return <span className="text-caption text-muted-foreground">{t(($) => $.page.integrations_server_managed)}</span>;
  }
  if (!canManage) {
    return <span className="text-caption text-muted-foreground">{t(($) => $.page.integrations_admin_only)}</span>;
  }
  if (query.isLoading || query.isError || !query.data) {
    return <span className="text-caption text-muted-foreground">{t(($) => $.page.integrations_unavailable)}</span>;
  }
  return (
    <Button type="button" variant="outline" size="sm" onClick={onOpen}>
      <Settings2 />
      {installedRecord(query.data)
        ? t(($) => $.page.integrations_manage)
        : t(($) => $.page.integrations_configure)}
    </Button>
  );
}

export function IntegrationsTab({
  standalone = false,
}: {
  standalone?: boolean;
} = {}) {
  const { t } = useT("settings");
  const workspace = useCurrentWorkspace();
  const wsId = workspace?.id ?? "";
  const user = useAuthStore((state) => state.user);
  const { data: members = [] } = useQuery({
    ...memberListOptions(wsId),
    enabled: !!wsId,
  });
  const currentMember = members.find((member) => member.user_id === user?.id);
  const canManage =
    currentMember?.role === "owner" || currentMember?.role === "admin";
  const isGuest = user?.is_guest === true;
  const messaging = useConfigStore((state) => state.messaging);
  const setupWritable = messaging?.setupWritable === true;
  const [managedChannel, setManagedChannel] = useState<MessagingChannel | null>(null);

  const lark = useQuery({ ...larkInstallationsOptions(wsId), enabled: !!wsId });
  const slack = useQuery({ ...slackInstallationsOptions(wsId), enabled: !!wsId });
  const dingtalk = useQuery({ ...dingtalkInstallationsOptions(wsId), enabled: !!wsId });
  const wecom = useQuery({ ...wecomInstallationsOptions(wsId), enabled: !!wsId });
  const telegram = useQuery({ ...telegramInstallationsOptions(wsId), enabled: !!wsId });
  const weixin = useQuery({ ...weixinInstallationsOptions(wsId), enabled: !!wsId });
  const messagingQuota = useQuery({
    queryKey: ["messaging-quota", wsId],
    queryFn: () => api.getMessagingQuotaUsage(wsId),
    enabled: !!wsId && messaging?.mode === "managed",
    staleTime: 30_000,
  });
  const quotaConsumed =
    messagingQuota.data?.used != null
      ? messagingQuota.data.used + (messagingQuota.data.reserved ?? 0)
      : null;
  const listings: Record<MessagingChannel, IntegrationQuery> = {
    lark,
    slack,
    dingtalk,
    wecom,
    telegram,
    weixin,
  };

  const linearEnabled = useFeatureEnabled(
    LINEAR_INSTALLATION_FOUNDATION_FLAG,
    false,
  );
  const composioEnabled = useFeatureEnabled(COMPOSIO_MCP_APPS_FLAG, false);
  const composioToolkits = useQuery({
    ...composioToolkitsOptions(),
    enabled: composioEnabled,
  });
  const composioUnconfigured =
    composioToolkits.error instanceof ApiError && composioToolkits.error.status === 503;
  const vcsAvailable = useConfigStore((state) => state.vcsIntegrationAvailable);

  function managedContent(channel: MessagingChannel) {
    const listing = listings[channel].data;
    if (installedRecord(listing)) {
      return {
        lark: <LarkTab />,
        slack: <SlackTab />,
        dingtalk: <DingTalkTab />,
        wecom: <WecomTab />,
        telegram: <TelegramTab />,
        weixin: <WeixinTab />,
      }[channel];
    }
    if (channel === "slack" && messaging?.mode === "managed") {
      return <SlackTab />;
    }
    const installAction = {
      lark: <LarkAgentBindButton />,
      slack: <SlackAgentBindButton />,
      dingtalk: <DingTalkAgentBindButton />,
      wecom: <WecomAgentBindButton />,
      telegram: <TelegramAgentBindButton />,
      weixin: <WeixinAgentBindButton />,
    }[channel];
    return (
      <div className="space-y-5">
        <IntegrationSetupGuide
          channel={channel}
          managed={messaging?.mode === "managed"}
        />
        {installAction}
      </div>
    );
  }

  const content = (
    <>
      <section className="space-y-4">
        {messaging?.mode === "managed" ? (
          <div className="text-caption text-muted-foreground" data-testid="messaging-quota">
            <span className="font-medium text-foreground">
              {t(($) => $.page.integrations_quota_title)}: {" "}
            </span>
            {messagingQuota.isLoading ? (
              t(($) => $.page.integrations_quota_loading)
            ) : messagingQuota.isError || messagingQuota.data?.mode === "unavailable" ? (
              t(($) => $.page.integrations_quota_unavailable)
            ) : messagingQuota.data?.mode === "unlimited" ? (
              t(($) => $.page.integrations_quota_unlimited)
            ) : messagingQuota.data?.mode === "managed" &&
              messagingQuota.data.limit != null && quotaConsumed != null ? (
              t(($) => $.page.integrations_quota_used, {
                used: quotaConsumed,
                limit: messagingQuota.data.limit,
              })
            ) : (
              t(($) => $.page.integrations_quota_unavailable)
            )}
          </div>
        ) : null}
        <div className="grid gap-4 md:grid-cols-2 xl:grid-cols-3">
          {([
            ["lark", t(($) => $.lark.section_title), t(($) => $.lark.page_description), "bg-[#3370FF]/10"],
            ["slack", t(($) => $.slack.section_title), t(($) => $.slack.page_description), "bg-[#611f69]/10"],
            ["dingtalk", t(($) => $.dingtalk.section_title), t(($) => $.dingtalk.page_description), "bg-[#1677FF]/10"],
            ["wecom", t(($) => $.wecom.section_title), t(($) => $.wecom.page_description), "bg-[#07C160]/10"],
            ["telegram", t(($) => $.telegram.section_title), t(($) => $.telegram.page_description), "bg-[#2AABEE]/10"],
            ["weixin", t(($) => $.weixin.section_title), t(($) => $.weixin.page_description), "bg-[#07C160]/10"],
          ] as const).map(([channel, title, description, iconClassName]) => (
            <IntegrationCard
              key={channel}
              channel={channel}
              title={title}
              description={description}
              iconClassName={iconClassName}
              status={<ChannelStatus query={listings[channel]} />}
              action={
                <ChannelAction
                  canManage={canManage}
                  isGuest={isGuest}
                  setupWritable={setupWritable}
                  query={listings[channel]}
                  onOpen={() => setManagedChannel(channel)}
                />
              }
            />
          ))}
          {linearEnabled ? (
            <LinearIntegrationCard
              canManage={canManage}
              isGuest={isGuest}
              workspaceId={wsId}
            />
          ) : null}
        </div>
      </section>

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
          <SettingsTab
            title={t(($) => $.page.integrations_title)}
            description={t(($) => $.page.integrations_description)}
          >
            {content}
          </SettingsTab>
        </div>
      ) : (
        <SettingsTab
          title={t(($) => $.page.tabs.integrations)}
          description={t(($) => $.page.integrations_description)}
        >
          {content}
        </SettingsTab>
      )}

      <Dialog
        open={managedChannel !== null}
        onOpenChange={(open) => !open && setManagedChannel(null)}
      >
        <DialogContent className="max-h-[90vh] overflow-y-auto sm:max-w-3xl">
          <DialogHeader>
            <DialogTitle>
              {managedChannel && installedRecord(listings[managedChannel].data)
                ? t(($) => $.page.integrations_manage)
                : t(($) => $.page.integrations_setup_title)}
            </DialogTitle>
          </DialogHeader>
          {managedChannel ? managedContent(managedChannel) : null}
        </DialogContent>
      </Dialog>
    </>
  );
}
