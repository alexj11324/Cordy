"use client";

import { useQuery } from "@tanstack/react-query";
import { skillListOptions } from "@orvilo/core/workspace/queries";
import { useCurrentWorkspace, useWorkspacePaths } from "@orvilo/core/paths";
import type { SkillSummary } from "@orvilo/core/types";
import { Button } from "@orvilo/ui/components/ui/button";
import { AppLink } from "../../navigation";
import { useT } from "../../i18n";
import {
  SettingsCard,
  SettingsEmpty,
  SettingsListRow,
  SettingsSection,
  SettingsTab,
} from "./settings-layout";

/**
 * Workspace skill library. Claim injects every skill in this workspace into
 * every agent — there is no per-agent assignment on this screen.
 */
export function SkillsTab() {
  const { t } = useT("settings");
  const workspace = useCurrentWorkspace();
  const wsId = workspace?.id ?? "";
  const paths = useWorkspacePaths();
  const skillsQuery = useQuery(skillListOptions(wsId));
  const skills = skillsQuery.data ?? [];

  return (
    <SettingsTab
      title={t(($) => $.skills.title)}
      description={t(($) => $.skills.description)}
      action={
        <Button
          variant="outline"
          size="sm"
          nativeButton={false}
          render={<AppLink href={paths.skills()} />}
        >
          {t(($) => $.skills.open_library)}
        </Button>
      }
    >
      <SettingsSection title={t(($) => $.skills.library_title)}>
        <SettingsCard>
          {skillsQuery.isLoading ? (
            <p className="px-4 py-6 text-body text-muted-foreground">
              {t(($) => $.skills.loading)}
            </p>
          ) : skills.length === 0 ? (
            <SettingsEmpty
              title={t(($) => $.skills.empty_title)}
              description={t(($) => $.skills.empty_description)}
            />
          ) : (
            skills.map((skill) => <SkillLibraryRow key={skill.id} skill={skill} />)
          )}
        </SettingsCard>
      </SettingsSection>
    </SettingsTab>
  );
}

function SkillLibraryRow({ skill }: { skill: SkillSummary }) {
  return (
    <SettingsListRow>
      <div className="min-w-0 flex-1">
        <p className="truncate text-body font-medium">{skill.name}</p>
        {skill.description ? (
          <p className="mt-0.5 line-clamp-2 text-caption text-muted-foreground">
            {skill.description}
          </p>
        ) : null}
      </div>
    </SettingsListRow>
  );
}
