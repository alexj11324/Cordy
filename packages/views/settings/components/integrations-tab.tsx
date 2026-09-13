"use client";

import { useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { CircleAlert, Loader2, Settings2Icon } from "lucide-react";
import { ApiError, api } from "@orvilo/core/api";
import { useAuthStore } from "@orvilo/core/auth";
import { workspaceSubscriptionSummaryOptions } from "@orvilo/core/billing";
import { composioToolkitsOptions } from "@orvilo/core/composio";
import { useConfigStore, useFeatureEnabled } from "@orvilo/core/config";
import { dingtalkInstallationsOptions } from "@orvilo/core/dingtalk";
import {
  BILLING_WORKSPACE_SUBSCRIPTIONS_FLAG,
  COMPOSIO_MCP_APPS_FLAG,
  LINEAR_INSTALLATION_FOUNDATION_FLAG,
} from "@orvilo/core/feature-flags";
import { larkInstallationsOptions } from "@orvilo/core/lark";
import { useCurrentWorkspace } from "@orvilo/core/paths";
import { slackInstallationsOptions } from "@orvilo/core/slack";
import { telegramInstallationsOptions } from "@orvilo/core/telegram";
import type { MessagingConnectionSource } from "@orvilo/core/types";
import { wecomInstallationsOptions } from "@orvilo/core/wecom";
import { weixinInstallationsOptions } from "@orvilo/core/weixin";
import { memberListOptions } from "@orvilo/core/workspace/queries";
import { Modal } from "@lobehub/ui/base-ui";
import { Badge } from "@orvilo/ui/components/reui/badge";
import { Frame, FramePanel } from "@orvilo/ui/components/reui/frame";
import { useT } from "../../i18n";
import { ComposioTab } from "./composio-tab";
import { DingTalkAgentBindButton, DingTalkTab } from "./dingtalk-tab";
import { IntegrationCard } from "./integration-card";
import type { IntegrationChannel } from "./integration-channel-icon";
import { IntegrationSetupGuide } from "./integration-setup-guide";
import { ConnectionDotBadge, IntegrationRowMenu } from "./integration-row-chrome";
import { LarkAgentBindButton, LarkTab } from "./lark-tab";
import { LinearIntegrationCard } from "./linear-tab";
import { MessagingConnectionStatus } from "./messaging-connection-status";
import { MessagingSetupNotice, useMessagingSetupWritable } from "./messaging-setup-policy";
import { SettingsGroup } from "./settings-shell";
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
  managed_supported?: boolean;
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
      <Badge variant="outline">
        <Loader2 className="animate-spin" />
        {t(($) => $.page.integrations_loading)}
      </Badge>
    );
  }
  if (query.isError || !query.data) {
    return (
      <Badge variant="destructive-light">
        <CircleAlert />
        {t(($) => $.page.integrations_unavailable)}
      </Badge>
    );
  }
  const hub = installedWorkspaceHub(query.data);
  if (hub) return <MessagingConnectionStatus installation={hub} compact />;
  if (installedRecord(query.data)) {
    return (
      <ConnectionDotBadge connected={false}>
        {t(($) => $.page.integrations_existing_agent)}
      </ConnectionDotBadge>
    );
  }
  if (!query.data.configured) {
    return (
      <ConnectionDotBadge connected={false}>
        {t(($) => $.page.integrations_setup_required)}
      </ConnectionDotBadge>
    );
  }
  return (
    <ConnectionDotBadge connected={false}>
      {t(($) => $.page.integrations_disconnected)}
    </ConnectionDotBadge>
  );
}

function ChannelActionMenu({
  actionLabel,
  onOpen,
}: {
  actionLabel: string;
  onOpen: () => void;
}) {
  return (
    <IntegrationRowMenu
      ariaLabel={actionLabel}
      items={[{ label: actionLabel, icon: Settings2Icon, onSelect: onOpen }]}
    />
  );
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
    return <span className="text-body text-muted-foreground">{t(($) => $.page.integrations_login_required)}</span>;
  }
  if (query.data?.installations.length) {
    return (
      <ChannelActionMenu
        actionLabel={
          canManage && setupWritable
            ? t(($) => $.page.integrations_manage)
            : t(($) => $.page.integrations_view_details)
        }
        onOpen={onOpen}
      />
    );
  }
  if (!setupWritable) {
    return (
      <ChannelActionMenu
        actionLabel={t(($) => $.page.integrations_view_setup)}
        onOpen={onOpen}
      />
    );
  }
  if (!canManage) {
    return <span className="text-body text-muted-foreground">{t(($) => $.page.integrations_admin_only)}</span>;
  }
  if (query.isLoading || query.isError || !query.data) {
    return <span className="text-body text-muted-foreground">{t(($) => $.page.integrations_unavailable)}</span>;
  }
  return (
    <ChannelActionMenu
      actionLabel={
        installedRecord(query.data)
          ? t(($) => $.page.integrations_manage)
          : t(($) => $.page.integrations_configure)
      }
      onOpen={onOpen}
    />
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
  const billingEnabled = useFeatureEnabled(
    BILLING_WORKSPACE_SUBSCRIPTIONS_FLAG,
    false,
  );
  const setupWritable = useMessagingSetupWritable();
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
  const subscriptionSummary = useQuery({
    ...workspaceSubscriptionSummaryOptions(wsId),
    enabled: !!wsId && billingEnabled,
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
  const hostedEntitlement = subscriptionSummary.data?.entitlement;
  const installationLimit = hostedEntitlement?.imInstallationLimit;
  const workspaceLimit = hostedEntitlement?.hostedWorkspaceLimit;
  const installedCount = Object.values(listings).reduce(
    (count, query) =>
      count +
      (query.data?.installations.filter(
        (installation) => installation.status === "installed",
      ).length ?? 0),
    0,
  );

  const linearEnabled = useFeatureEnabled(
    LINEAR_INSTALLATION_FOUNDATION_FLAG,
    true,
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
    const hasInstallations = (listing?.installations.length ?? 0) > 0;
    const hasWorkspaceHub = !!installedWorkspaceHub(listing);
    const slackOAuth = channel === "slack" && listing?.managed_supported === true;
    const canStartSetup = setupWritable && canManage && !isGuest && !hasWorkspaceHub;
    const installSupported = listing?.configured === true &&
      (slackOAuth || listing.install_supported === true) && !listings[channel].isError;
    const platformDetails = {
      lark: <LarkTab />,
      slack: <SlackTab />,
      dingtalk: <DingTalkTab />,
      wecom: <WecomTab />,
      telegram: <TelegramTab />,
      weixin: <WeixinTab />,
    }[channel];
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
        {!setupWritable && !hasInstallations && <MessagingSetupNotice />}
        {canStartSetup && (
          <>
            <IntegrationSetupGuide channel={channel} managed={slackOAuth} />
            {installSupported
              ? slackOAuth ? platformDetails : installAction
              : <MessagingSetupNotice />}
          </>
        )}
        {hasInstallations && !(canStartSetup && installSupported && slackOAuth) && platformDetails}
      </div>
    );
  }

  const content = (
    <>
      <section className="@container space-y-4">
        {messaging?.mode === "managed" ? (
          <div className="space-y-1 text-caption text-muted-foreground" data-testid="messaging-quota">
            <div>
            <span className="font-medium text-foreground">
              {t(($) => $.page.integrations_quota_title)}: {" "}
            </span>
            {messagingQuota.isLoading ? (
              t(($) => $.page.integrations_quota_loading)
            ) : messagingQuota.isError || messagingQuota.data?.mode === "unavailable" ? (
              t(($) => $.page.integrations_quota_unavailable)
            ) : messagingQuota.data?.mode === "unlimited" ||
              messagingQuota.data?.mode === "disabled" ? (
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
            {billingEnabled ? (
              <div className="flex flex-wrap gap-x-3 gap-y-1">
                <span>
                  {t(($) => $.page.integrations_installation_quota)}: {" "}
                  {subscriptionSummary.isLoading
                    ? t(($) => $.page.integrations_quota_loading)
                    : subscriptionSummary.isError || !hostedEntitlement
                      ? t(($) => $.page.integrations_quota_unavailable)
                      : installationLimit === null
                        ? t(($) => $.page.integrations_quota_unlimited)
                        : typeof installationLimit === "number"
                          ? t(($) => $.page.integrations_installation_quota_used, {
                              used: installedCount,
                              limit: installationLimit,
                            })
                          : t(($) => $.page.integrations_quota_unavailable)}
                </span>
                <span>
                  {t(($) => $.page.integrations_workspace_quota)}: {" "}
                  {subscriptionSummary.isLoading
                    ? t(($) => $.page.integrations_quota_loading)
                    : subscriptionSummary.isError || !hostedEntitlement
                      ? t(($) => $.page.integrations_quota_unavailable)
                      : workspaceLimit === null
                        ? t(($) => $.page.integrations_quota_unlimited)
                        : typeof workspaceLimit === "number"
                          ? t(($) => $.page.integrations_workspace_quota_limit, {
                              limit: workspaceLimit,
                            })
                          : t(($) => $.page.integrations_quota_unavailable)}
                </span>
              </div>
            ) : null}
          </div>
        ) : null}
        <Frame className="w-full">
          <FramePanel className="space-y-2">
            <div>
              <h2 className="text-body font-semibold">{t(($) => $.page.integrations_group_communication)}</h2>
              <p className="text-body text-muted-foreground">
                {t(($) => $.page.integrations_group_communication_description)}
              </p>
            </div>
            <div>
              {([
                ["lark", t(($) => $.lark.section_title), t(($) => $.lark.page_description)],
                ["slack", t(($) => $.slack.section_title), t(($) => $.slack.page_description)],
                ["dingtalk", t(($) => $.dingtalk.section_title), t(($) => $.dingtalk.page_description)],
                ["wecom", t(($) => $.wecom.section_title), t(($) => $.wecom.page_description)],
                ["telegram", t(($) => $.telegram.section_title), t(($) => $.telegram.page_description)],
                ["weixin", t(($) => $.weixin.section_title), t(($) => $.weixin.page_description)],
              ] as const).map(([channel, title, description]) => (
                <IntegrationCard
                  key={channel}
                  channel={channel}
                  title={title}
                  description={description}
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
            </div>
          </FramePanel>
          {linearEnabled ? (
            <FramePanel className="space-y-2">
              <div>
                <h2 className="text-body font-semibold">{t(($) => $.page.integrations_group_collaboration)}</h2>
                <p className="text-body text-muted-foreground">
                  {t(($) => $.page.integrations_group_collaboration_description)}
                </p>
              </div>
              <div>
                <LinearIntegrationCard
                  canManage={canManage}
                  isGuest={isGuest}
                  workspaceId={wsId}
                />
              </div>
            </FramePanel>
          ) : null}
        </Frame>
      </section>

      {/* `SettingsSection` without a `SettingsCard` is a *bare* section, and a
          bare `Form.Group` is borderless — Lobe's own default. Using the
          outlined variant here would have put `ComposioTab`'s already-migrated
          card grid inside a second bordered panel. */}
      {composioEnabled && !composioUnconfigured ? (
        <SettingsGroup variant="borderless" title={t(($) => $.composio.section_title)}>
          <ComposioTab />
        </SettingsGroup>
      ) : null}
      {vcsAvailable ? (
        <SettingsGroup variant="borderless" title={t(($) => $.vcs.section_title)}>
          <VCSTab />
        </SettingsGroup>
      ) : null}
    </>
  );

  return (
    <>
      {/* `SettingsTab` is gone, and with it its `nestedInDialog` branch, which
          returned before reading `title`/`description`/`action` — inside the
          settings dialog the page title is the shell's `DialogHeader`, so
          dropping it there is the mapping. The standalone Web route
          (`WorkspaceIntegrationsPage`) has no shell, so it keeps its own
          heading; that branch passes `integrations_title`, and the two keys
          resolve to the same string in both locales. Neither branch has an
          `action` to move to `extra`, so nothing was lost.

          The spacing the discarded branch supplied (`space-y-12`) is now this
          container's, at the family convention of `space-y-8`.

          The heading is the exception, and `mb-12` below is why. The baseline
          put the standalone title in a `<header className="mb-12 …">` — so
          heading → body was 48px — and `space-y-8` alone renders 32. It is not
          additive: `space-y-8` writes `margin-bottom` on the *earlier* sibling
          (`:not(:last-child)`), so a `mb-*` on that heading replaces its 32
          rather than stacking with it, and the value has to be the full 48.
          The standalone wrapper's own `space-y-8` never supplied this spacing:
          it held one child, so the rule was inert on it. */}
      <div
        className={
          standalone
            ? "mx-auto w-full max-w-6xl space-y-8 p-4 sm:p-6 lg:p-8"
            : "space-y-8"
        }
      >
        {standalone ? (
          <h2 className="text-display-sm font-semibold tracking-tight mb-12">
            {t(($) => $.page.integrations_title)}
          </h2>
        ) : null}
        {content}
      </div>

      {/* The channel dialog. `sm:max-w-3xl` was 768px; the Modal owns its own
          max-height and scrolling, so the old `max-h-[90vh] overflow-y-auto`
          is gone. `destroyOnHidden` is deliberately absent: this is base-ui's
          `Modal`, which destructures a fixed prop list and drops it. */}
      <Modal
        // `footer={null}` is a regression fix, not a preference. Lobe's `Modal`
        // builds `cancelBtnNode + okBtnNode` whenever `footer` is left
        // undefined (`es/base-ui/Modal/Modal.mjs`), and the OK button calls
        // `onOk` — which this dialog does not pass, so the OK it rendered did
        // nothing at all. The pre-migration dialog had `DialogContent` with no
        // `DialogFooter` (`git show 28b76064:…/integrations-tab.tsx`), so the
        // footer was added by this migration and half of it was dead.
        footer={null}
        open={managedChannel !== null}
        title={
          managedChannel && installedRecord(listings[managedChannel].data)
            ? canManage && setupWritable
              ? t(($) => $.page.integrations_manage)
              : t(($) => $.page.integrations_view_details)
            : t(($) => $.page.integrations_setup_title)
        }
        width={768}
        onCancel={() => setManagedChannel(null)}
      >
        {managedChannel ? managedContent(managedChannel) : null}
      </Modal>
    </>
  );
}
