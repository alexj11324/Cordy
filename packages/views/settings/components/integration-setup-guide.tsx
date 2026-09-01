"use client";

import { ExternalLink } from "lucide-react";
import { Button } from "@patchbay/ui/components/ui/button";
import { openExternal } from "../../platform";
import { useT } from "../../i18n";
import type { IntegrationChannel } from "./integration-channel-icon";
import { slackDocsUrl } from "./slack-docs-url";

const providerConsoleUrls: Partial<Record<Exclude<IntegrationChannel, "linear">, string>> = {
  slack: "https://api.slack.com/apps",
  dingtalk: "https://open-dev.dingtalk.com",
  wecom: "https://work.weixin.qq.com/wework_admin/frame#apps",
  telegram: "https://t.me/BotFather",
};

export function IntegrationSetupGuide({
  channel,
  managed = false,
}: {
  channel: Exclude<IntegrationChannel, "linear">;
  managed?: boolean;
}) {
  const { t, i18n } = useT("settings");
  const copy = {
    lark: {
      requirement: t(($) => $.lark.setup_requirement),
      steps: [
        t(($) => $.lark.setup_step_1),
        t(($) => $.lark.setup_step_2),
        t(($) => $.lark.setup_step_3),
      ],
      open: t(($) => $.lark.setup_open),
    },
    slack: {
      requirement: managed
        ? t(($) => $.slack.managed_setup_requirement)
        : t(($) => $.slack.setup_requirement),
      steps: managed
        ? [
            t(($) => $.slack.managed_setup_step_1),
            t(($) => $.slack.managed_setup_step_2),
            t(($) => $.slack.managed_setup_step_3),
          ]
        : [
            t(($) => $.slack.setup_step_1),
            t(($) => $.slack.setup_step_2),
            t(($) => $.slack.setup_step_3),
          ],
      open: t(($) => $.slack.setup_open),
    },
    dingtalk: {
      requirement: t(($) => $.dingtalk.setup_requirement),
      steps: [
        t(($) => $.dingtalk.setup_step_1),
        t(($) => $.dingtalk.setup_step_2),
        t(($) => $.dingtalk.setup_step_3),
      ],
      open: t(($) => $.dingtalk.setup_open),
    },
    wecom: {
      requirement: t(($) => $.wecom.setup_requirement),
      steps: [
        t(($) => $.wecom.setup_step_1),
        t(($) => $.wecom.setup_step_2),
        t(($) => $.wecom.setup_step_3),
      ],
      open: t(($) => $.wecom.setup_open),
    },
    telegram: {
      requirement: t(($) => $.telegram.setup_requirement),
      steps: [
        t(($) => $.telegram.setup_step_1),
        t(($) => $.telegram.setup_step_2),
        t(($) => $.telegram.setup_step_3),
      ],
      open: t(($) => $.telegram.setup_open),
    },
    weixin: {
      requirement: t(($) => $.weixin.setup_requirement),
      steps: [
        t(($) => $.weixin.setup_step_1),
        t(($) => $.weixin.setup_step_2),
        t(($) => $.weixin.setup_step_3),
      ],
      open: t(($) => $.weixin.setup_open),
    },
  }[channel];
  const consoleUrl = managed && channel === "slack" ? undefined : providerConsoleUrls[channel];
  const instructionsUrl =
    channel === "slack" && !managed ? slackDocsUrl(i18n.language) : undefined;

  return (
    <section
      className="space-y-5 rounded-xl border bg-muted/20 p-5"
      data-testid={`integration-setup-guide-${channel}`}
    >
      <div className="space-y-1.5">
        <h3 className="text-body font-semibold">
          {t(($) => $.page.integrations_requirements_title)}
        </h3>
        <p className="text-caption leading-5 text-muted-foreground">{copy.requirement}</p>
      </div>
      <div className="space-y-2">
        <h3 className="text-body font-semibold">
          {t(($) => $.page.integrations_setup_steps_title)}
        </h3>
        <ol className="space-y-2 pl-5 text-caption leading-5 text-muted-foreground">
          {copy.steps.map((step) => (
            <li key={step} className="list-decimal pl-1">
              {step}
            </li>
          ))}
        </ol>
      </div>
      <div className="flex flex-wrap gap-2">
        {instructionsUrl ? (
          <Button
            type="button"
            variant="outline"
            size="sm"
            onClick={() => openExternal(instructionsUrl)}
          >
            <ExternalLink className="size-3.5" />
            {t(($) => $.slack.setup_manifest_open)}
          </Button>
        ) : null}
        {consoleUrl ? (
          <Button
            type="button"
            variant="outline"
            size="sm"
            onClick={() => openExternal(consoleUrl)}
          >
            <ExternalLink className="size-3.5" />
            {copy.open}
          </Button>
        ) : null}
      </div>
    </section>
  );
}
