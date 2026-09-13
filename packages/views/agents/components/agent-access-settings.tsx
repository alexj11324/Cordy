"use client";

import type { Agent, MemberWithUser } from "@orvilo/core/types";
import { SettingsGroup } from "../../settings/components/settings-shell";
import { useT } from "../../i18n";
import { AccessPicker } from "./inspector/access-picker";

export function AgentAccessSettings({
  agent,
  members,
  currentUserId,
  onDirtyChange,
  onUpdate,
}: {
  agent: Agent;
  members: MemberWithUser[];
  currentUserId: string | null;
  onDirtyChange?: (dirty: boolean) => void;
  onUpdate: (id: string, data: Record<string, unknown>) => Promise<void>;
}) {
  const { t } = useT("agents");

  return (
    <SettingsGroup
      title={t(($) => $.access.section_title)}
      description={t(($) => $.inspector.section_access_hint)}
    >
      {/* `AccessPicker` is a self-contained card body: its scope rows carry
          `px-4` (`inspector/access-picker.tsx:369`), so the group body's own
          16px inset has to be cancelled or every row lands 16px right of the
          group's title. `-mx-4` is that cancellation, and it is the whole of
          it: the rows carry `px-4` with no responsive variant, so 16px is the
          only inset there is to match.

          It read `-mx-4 sm:-mx-6` before this. The `sm:` half matched nothing
          the rows do, and at 1280px it put every horizontal rule the picker
          paints 7px outside the card's own border — measured at that width:
          rules at x=281..843 against a card border at 288..836, and the rows'
          content at x=297 against the group title's x=305. */}
      <div className="-mx-4">
        <AccessPicker
          permissionMode={agent.permission_mode}
          invocationTargets={agent.invocation_targets}
          visibility={agent.visibility}
          members={members}
          ownerId={agent.owner_id}
          canEdit={
            currentUserId !== null && agent.owner_id === currentUserId
          }
          hasComposioAllowlist={
            (agent.composio_toolkit_allowlist ?? []).length > 0
          }
          onDirtyChange={onDirtyChange}
          onChange={(next) => onUpdate(agent.id, next)}
        />
      </div>
    </SettingsGroup>
  );
}
