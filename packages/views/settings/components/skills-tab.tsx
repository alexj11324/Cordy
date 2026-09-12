"use client";

import { useQuery } from "@tanstack/react-query";
import { skillListOptions } from "@orvilo/core/workspace/queries";
import { useCurrentWorkspace, useWorkspacePaths } from "@orvilo/core/paths";
import type { SkillSummary } from "@orvilo/core/types";
import { Button } from "@orvilo/ui/components/ui/button";
import { cn } from "@orvilo/ui/lib/utils";
import { AppLink } from "../../navigation";
import { useT } from "../../i18n";
import { SettingsEmptyState } from "./settings-empty";
import { SettingsGroup } from "./settings-shell";

/**
 * Workspace skill library. Claim injects every skill in this workspace into
 * every agent — there is no per-agent assignment on this screen.
 *
 * **`SettingsTab` is gone, and this tab is where losing it is visible: the
 * panel's page-level action was dropped inside the settings dialog.**
 * `SettingsTab`'s nested branch returns `children` and nothing else
 * (`settings-layout.tsx`), and it is always taken there, so the `<Button>` that
 * used to open the Skills library never rendered in the dialog — the panel had
 * **zero** buttons. It lives on the group's `extra` now, which renders on both
 * branches. (The Skills route has two other entries — `app-sidebar.tsx`'s
 * `configureNav` and `global-shortcuts.tsx`'s `goSkills` — so what was lost is a
 * page-level control inside one panel, not the surface.)
 *
 * **That control is the one place this file keeps the shadcn `Button`.** Lobe's
 * base-ui `Button` has no `render` slot, and this control's whole implementation
 * is the composition `render={<AppLink …/>}`: `AppLink` owns the platform
 * routing, the modifier-click intents and the desktop renderer's `file://`
 * href problem. Lobe's `Button` *does* render an anchor when given `href`, but
 * that route is a behaviour regression on both clients — a bare `/slug/skills`
 * href navigates the Electron renderer off the app, and intercepting it with
 * `push` in `onClick` throws away the new-tab path `AppLink` exists to keep.
 * Reimplementing either is what `AppLink` is for, so the trade is one 28px
 * shadcn button on an otherwise-Lobe panel. Reported as a deviation, not a
 * silent carry-over.
 *
 * **`title` must not come back either.** `skills.title` resolves to "Skills" /
 * "Skills", the identical string `page.tabs.skills` puts in the dialog header,
 * so a group carrying it would print the page title twice — the defect Task 6
 * shipped in `labels-tab`. The group title is `skills.library_title`
 * ("Workspace skills" / "工作区 Skills"), which is a section name.
 *
 * The root is `space-y-8`: the lede and the group are two visible sibling
 * blocks, and `SettingsTab`'s nested branch is what used to separate them.
 */
export function SkillsTab() {
  const { t } = useT("settings");
  const workspace = useCurrentWorkspace();
  const wsId = workspace?.id ?? "";
  const paths = useWorkspacePaths();
  const skillsQuery = useQuery(skillListOptions(wsId));
  const skills = skillsQuery.data ?? [];

  return (
    <div className="space-y-8">
      <p className="text-body text-muted-foreground">{t(($) => $.skills.description)}</p>

      <SettingsGroup
        extra={
          <Button
            nativeButton={false}
            render={<AppLink href={paths.skills()} />}
            size="sm"
            variant="outline"
          >
            {t(($) => $.skills.open_library)}
          </Button>
        }
        title={t(($) => $.skills.library_title)}
      >
        {skillsQuery.isLoading ? (
          <SettingsEmptyState title={t(($) => $.skills.loading)} />
        ) : skills.length === 0 ? (
          <SettingsEmptyState
            description={t(($) => $.skills.empty_description)}
            title={t(($) => $.skills.empty_title)}
          />
        ) : (
          skills.map((skill, index) => (
            <SkillLibraryRow key={skill.id} divider={index > 0} skill={skill} />
          ))
        )}
      </SettingsGroup>
    </div>
  );
}

function SkillLibraryRow({
  skill,
  divider,
}: {
  skill: SkillSummary;
  divider: boolean;
}) {
  // A skill is a name with its description under it, not a label/control pair,
  // so there is no `Form.Item` here — the retired `SettingsListRow` is inlined
  // instead of re-homed, the same shape `repositories-tab` uses for a project
  // row. The group panel already supplies the `px-4` the old card row carried,
  // and the row draws the `divide-y` the old `SettingsCard` drew.
  return (
    <div
      className={cn(
        "flex min-h-16 items-center gap-4 py-3 text-body",
        divider && "border-t border-border",
      )}
    >
      <div className="min-w-0 flex-1">
        <p className="truncate text-body font-medium">{skill.name}</p>
        {skill.description ? (
          <p className="mt-0.5 line-clamp-2 text-caption text-muted-foreground">
            {skill.description}
          </p>
        ) : null}
      </div>
    </div>
  );
}
