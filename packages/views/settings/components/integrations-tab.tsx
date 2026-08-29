"use client";

import type { ReactNode } from "react";
import { useQuery } from "@tanstack/react-query";
import { Loader2 } from "lucide-react";
import { Card, CardContent } from "@patchbay/ui/components/ui/card";
import { ApiError } from "@patchbay/core/api";
import { useConfigStore, useFeatureEnabled } from "@patchbay/core/config";
import { COMPOSIO_MCP_APPS_FLAG } from "@patchbay/core/feature-flags";
import { useAuthStore } from "@patchbay/core/auth";
import { useWorkspaceId } from "@patchbay/core/hooks";
import { memberListOptions } from "@patchbay/core/workspace/queries";
import { larkInstallationsOptions } from "@patchbay/core/lark";
import { slackInstallationsOptions } from "@patchbay/core/slack";
import { dingtalkInstallationsOptions } from "@patchbay/core/dingtalk";
import { wecomInstallationsOptions } from "@patchbay/core/wecom";
import { telegramInstallationsOptions } from "@patchbay/core/telegram";
import { weixinInstallationsOptions } from "@patchbay/core/weixin";
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
import { DingTalkAgentBindButton } from "./dingtalk-tab";
import { WecomAgentBindButton } from "./wecom-tab";
import { TelegramAgentBindButton } from "./telegram-tab";
import { WeixinAgentBindButton } from "./weixin-tab";

type InstallationSummary = {
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
  title: string;
};

type HubActionProps = {
  canManage: boolean;
  children: ReactNode;
  query: IntegrationQuery;
};

function hasActiveHub(listing: InstallationListing | undefined) {
  return (
    listing?.installations.some(
      (installation) =>
        installation.agent_id === null && installation.status === "active",
    ) ?? false
  );
}

function HubAction({ canManage, children, query }: HubActionProps) {
  const { t } = useT("settings");
  const hubConnected = hasActiveHub(query.data);

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
      <span className="text-caption text-muted-foreground">
        {t(($) => $.page.integrations_setup_required)}
      </span>
    );
  }
  if (!query.data.install_supported && !hubConnected) {
    return (
      <span className="text-caption text-muted-foreground">
        {t(($) => $.page.integrations_coming_soon)}
      </span>
    );
  }
  return children;
}

function IntegrationCard({
  action,
  channel,
  description,
  iconClassName,
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
export function IntegrationsTab() {
  const { t } = useT("settings");
  const wsId = useWorkspaceId();
  const user = useAuthStore((state) => state.user);
  const { data: members = [] } = useQuery({
    ...memberListOptions(wsId),
    enabled: !!wsId,
  });
  const currentMember = members.find((member) => member.user_id === user?.id);
  const canManage =
    currentMember?.role === "owner" || currentMember?.role === "admin";

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

  const composioEnabled = useFeatureEnabled(COMPOSIO_MCP_APPS_FLAG, false);
  const composioToolkits = useQuery({
    ...composioToolkitsOptions(),
    enabled: composioEnabled,
  });
  const composioUnconfigured =
    composioToolkits.error instanceof ApiError &&
    composioToolkits.error.status === 503;
  const vcsAvailable = useConfigStore((state) => state.vcsIntegrationAvailable);

  return (
    <SettingsTab
      title={t(($) => $.page.tabs.integrations)}
      description={t(($) => $.page.integrations_description)}
    >
      <section className="space-y-4">
        <div>
          <h3 className="text-body font-semibold">
            {t(($) => $.page.integrations_channels_title)}
          </h3>
          <p className="mt-1 text-caption leading-5 text-muted-foreground">
            {t(($) => $.page.integrations_channels_description)}
          </p>
        </div>
        <div className="grid gap-4 sm:grid-cols-2">
          <IntegrationCard
            channel="lark"
            title={t(($) => $.lark.section_title)}
            description={t(($) => $.lark.page_description)}
            iconClassName="bg-[#3370FF]/10"
            action={
              <HubAction canManage={canManage} query={lark}>
                <LarkAgentBindButton workspaceScoped />
              </HubAction>
            }
          />
          <IntegrationCard
            channel="slack"
            title={t(($) => $.slack.section_title)}
            description={t(($) => $.slack.page_description)}
            iconClassName="bg-[#611f69]/10"
            action={
              <HubAction canManage={canManage} query={slack}>
                <SlackAgentBindButton />
              </HubAction>
            }
          />
          <IntegrationCard
            channel="dingtalk"
            title={t(($) => $.dingtalk.section_title)}
            description={t(($) => $.dingtalk.page_description)}
            iconClassName="bg-[#1677FF]/10"
            action={
              <HubAction canManage={canManage} query={dingtalk}>
                <DingTalkAgentBindButton />
              </HubAction>
            }
          />
          <IntegrationCard
            channel="wecom"
            title={t(($) => $.wecom.section_title)}
            description={t(($) => $.wecom.page_description)}
            iconClassName="bg-[#07C160]/10"
            action={
              <HubAction canManage={canManage} query={wecom}>
                <WecomAgentBindButton />
              </HubAction>
            }
          />
          <IntegrationCard
            channel="telegram"
            title={t(($) => $.telegram.section_title)}
            description={t(($) => $.telegram.page_description)}
            iconClassName="bg-[#2AABEE]/10"
            action={
              <HubAction canManage={canManage} query={telegram}>
                <TelegramAgentBindButton />
              </HubAction>
            }
          />
          <IntegrationCard
            channel="weixin"
            title={t(($) => $.weixin.section_title)}
            description={t(($) => $.weixin.page_description)}
            iconClassName="bg-[#07C160]/10"
            action={
              <HubAction canManage={canManage} query={weixin}>
                <WeixinAgentBindButton />
              </HubAction>
            }
          />
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
    </SettingsTab>
  );
}
