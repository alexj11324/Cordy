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
      {/* `AccessPicker` is a self-contained card body: its own rows carry
          `px-4`/`sm:px-6`, so it needs the group's body padding cancelled or
          every scope row lands 16px right of the group's title. The negative
          margins match the row padding at each breakpoint rather than guessing
          one number. */}
      <div className="-mx-4 sm:-mx-6">
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
