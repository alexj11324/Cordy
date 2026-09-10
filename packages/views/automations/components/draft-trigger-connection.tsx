"use client";

import { useQuery } from "@tanstack/react-query";
import { ExternalLink } from "lucide-react";
import {
  isNativeAutomationProvider,
  settingsPathForTriggerProvider,
} from "@orvilo/core/automations";
import { githubInstallationsOptions } from "@orvilo/core/github/queries";
import { linearConnectionOptions } from "@orvilo/core/linear/queries";
import { slackInstallationsOptions } from "@orvilo/core/slack/queries";
import { useWorkspaceId } from "@orvilo/core/hooks";
import { useWorkspacePaths } from "@orvilo/core/paths";
import { Button } from "@orvilo/ui/components/ui/button";
import { useT } from "../../i18n";
import { AppLink } from "../../navigation";

export function DraftTriggerConnection({
  provider,
  disabled,
}: {
  provider: string | null | undefined;
  disabled: boolean;
}) {
  const { t } = useT("automations");
  const wsId = useWorkspaceId();
  const paths = useWorkspacePaths();
  const github = useQuery({
    ...githubInstallationsOptions(wsId),
    enabled: !!wsId && provider === "github",
    refetchOnWindowFocus: "always",
    refetchOnMount: "always",
  });
  const slack = useQuery({
    ...slackInstallationsOptions(wsId),
    enabled: !!wsId && provider === "slack",
    refetchOnWindowFocus: "always",
    refetchOnMount: "always",
  });
  const linear = useQuery({
    ...linearConnectionOptions(wsId),
    enabled: !!wsId && provider === "linear",
    refetchOnWindowFocus: "always",
    refetchOnMount: "always",
  });

  if (!isNativeAutomationProvider(provider)) return null;

  const connection =
    provider === "github" ? github : provider === "slack" ? slack : linear;
  if (connection.isPending) {
    return (
      <span role="status" className="text-caption text-muted-foreground">
        {t(($) => $.settings.checking_connection)}
      </span>
    );
  }
  if (connection.isError) {
    return (
      <div className="flex flex-wrap items-center gap-2">
        <span role="status" className="text-caption text-destructive">
          {t(($) => $.settings.connection_failed)}
        </span>
        <Button
          size="sm"
          variant="ghost"
          disabled={disabled}
          onClick={() => void connection.refetch()}
        >
          {t(($) => $.page.retry)}
        </Button>
      </div>
    );
  }
  const connected =
    provider === "github"
      ? (github.data?.installations.length ?? 0) > 0
      : provider === "slack"
        ? (slack.data?.installations ?? []).some(
            (row) => row.status === "installed" || row.installation_status === "installed",
          )
        : linear.data?.connected === true && linear.data.connection?.status === "active";
  if (connected) return null;

  return (
    <div className="flex shrink-0 flex-wrap items-center gap-2">
      <span className="text-caption text-amber-600 dark:text-amber-400">
        {t(($) => $.settings.tools_requires_connection)}
      </span>
      <Button
        size="sm"
        className="h-7 shrink-0 px-2.5"
        disabled={disabled}
        nativeButton={false}
        render={
          <AppLink
            href={settingsPathForTriggerProvider(paths.settings(), provider)}
            target="_blank"
            rel="noopener noreferrer"
          />
        }
      >
        {t(($) => $.settings.tools_connect)}
        <ExternalLink className="size-3.5" />
      </Button>
    </div>
  );
}
