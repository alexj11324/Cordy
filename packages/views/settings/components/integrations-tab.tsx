"use client";

import { useQuery } from "@tanstack/react-query";
import { LarkTab } from "./lark-tab";
import { ComposioTab } from "./composio-tab";
import { SlackTab } from "./slack-tab";
import { DingTalkTab } from "./dingtalk-tab";
import { VCSTab } from "./vcs-tab";
import { WecomTab } from "./wecom-tab";
import { WeixinTab } from "./weixin-tab";
import { TelegramTab } from "./telegram-tab";
import { LinearIntegrationCard } from "./linear-tab";
import { ApiError } from "@patchbay/core/api";
import { useAuthStore } from "@patchbay/core/auth";
import { composioToolkitsOptions } from "@patchbay/core/composio";
import { useCurrentWorkspace } from "@patchbay/core/paths";
import { memberListOptions } from "@patchbay/core/workspace/queries";
import { useConfigStore, useFeatureEnabled } from "@patchbay/core/config";
import {
  COMPOSIO_MCP_APPS_FLAG,
  LINEAR_INSTALLATION_FOUNDATION_FLAG,
} from "@patchbay/core/feature-flags";
import { useT } from "../../i18n";
import { SettingsSection, SettingsTab } from "./settings-layout";
import { IntegrationChannelIcon } from "./integration-channel-icon";

// Integrations is the umbrella tab for third-party platform connections.
// GitHub has its own top-level tab (see github-tab.tsx); everything else
// — currently Lark, Composio, Slack, Telegram, the self-hosted Git providers
// (Forgejo / Gitea / GitLab), and WeCom smart-bot, with Linear etc. to follow —
// lives in here under its own section heading so additional integrations slot
// in without changing the IA. IntegrationsTab is just the host; each
// integration owns its own description and install flow.
export function IntegrationsTab() {
  const { t } = useT("settings");
  const linearEnabled = useFeatureEnabled(
    LINEAR_INSTALLATION_FOUNDATION_FLAG,
    false,
  );
  const wsId = useCurrentWorkspace()?.id ?? "";
  const user = useAuthStore((state) => state.user);
  const members = useQuery({
    ...memberListOptions(wsId),
    enabled: linearEnabled && !!wsId,
  });
  const currentMember = members.data?.find((member) => member.user_id === user?.id);
  const canManage = currentMember?.role === "owner" || currentMember?.role === "admin";
  const composioEnabled = useFeatureEnabled(COMPOSIO_MCP_APPS_FLAG, false);
  const composioToolkits = useQuery({
    ...composioToolkitsOptions(),
    enabled: composioEnabled,
  });
  const composioUnconfigured =
    composioToolkits.error instanceof ApiError && composioToolkits.error.status === 503;

  // Self-host-only integration: the managed cloud reports this false (field
  // omitted from /api/config), so the whole section — header included — is
  // hidden there rather than showing an operator-only "missing key" message.
  const vcsAvailable = useConfigStore((s) => s.vcsIntegrationAvailable);

  return (
    <SettingsTab title={t(($) => $.page.tabs.integrations)}>
      <SettingsSection
        title={
          <span className="flex items-center gap-2">
            <IntegrationChannelIcon channel="lark" />
            {t(($) => $.lark.section_title)}
          </span>
        }
        description={t(($) => $.lark.page_description)}
      >
        <LarkTab />
      </SettingsSection>
      {composioEnabled && !composioUnconfigured && (
        <SettingsSection title={t(($) => $.composio.section_title)}>
          <ComposioTab />
        </SettingsSection>
      )}
      <SettingsSection
        title={
          <span className="flex items-center gap-2">
            <IntegrationChannelIcon channel="slack" />
            {t(($) => $.slack.section_title)}
          </span>
        }
        description={t(($) => $.slack.page_description)}
      >
        <SlackTab />
      </SettingsSection>
      <SettingsSection
        title={
          <span className="flex items-center gap-2">
            <IntegrationChannelIcon channel="dingtalk" />
            {t(($) => $.dingtalk.section_title)}
          </span>
        }
        description={t(($) => $.dingtalk.page_description)}
      >
        <DingTalkTab />
      </SettingsSection>
      {vcsAvailable && (
        <SettingsSection title={t(($) => $.vcs.section_title)}>
          <VCSTab />
        </SettingsSection>
      )}
      {linearEnabled && (
        <SettingsSection
          title={
            <span className="flex items-center gap-2">
              <IntegrationChannelIcon channel="linear" />
              {t(($) => $.page.linear.title)}
            </span>
          }
          description={t(($) => $.page.linear.description)}
        >
          <LinearIntegrationCard
            canManage={canManage}
            isGuest={!user}
            workspaceId={wsId}
          />
        </SettingsSection>
      )}
      <SettingsSection
        title={
          <span className="flex items-center gap-2">
            <IntegrationChannelIcon channel="wecom" />
            {t(($) => $.wecom.section_title)}
          </span>
        }
        description={t(($) => $.wecom.page_description)}
      >
        <WecomTab />
      </SettingsSection>
      <SettingsSection
        title={
          <span className="flex items-center gap-2">
            <IntegrationChannelIcon channel="weixin" />
            {t(($) => $.weixin.section_title)}
          </span>
        }
        description={t(($) => $.weixin.page_description)}
      >
        <WeixinTab />
      </SettingsSection>
      <SettingsSection
        title={
          <span className="flex items-center gap-2">
            <IntegrationChannelIcon channel="telegram" />
            {t(($) => $.telegram.section_title)}
          </span>
        }
        description={t(($) => $.telegram.page_description)}
      >
        <TelegramTab />
      </SettingsSection>
    </SettingsTab>
  );
}
