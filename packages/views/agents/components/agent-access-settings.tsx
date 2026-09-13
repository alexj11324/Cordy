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
          group's title. `-mx-4` is that cancellation. The rows are why it is
          16px: they carry `px-4` with no responsive variant. (Not every child
          of this wrapper is 16px — see below.)

          It read `-mx-4 sm:-mx-6` before this, and at 1280px that put every
          horizontal rule the picker paints 7px outside the card's own border —
          measured at that width: rules at x=281..843 against a card border at
          288..836, and the rows' content at x=297 against the group title's
          x=305.

          The `sm:` half was not dead code, and this wrapper is not the only
          `px-4`/`px-6` pair in play. Two blocks inside the picker carry a wider
          inset at the same breakpoint — `inspector/access-picker.tsx:270`
          (`px-4 py-5 sm:px-6`, the member list, whose empty state sits inside
          it at `:274`) and `:321` (`px-4 py-4 sm:px-6`, the Composio allowlist
          hint, which renders only when the allowlist is non-empty). Cancelling
          24px here while those children
          inset 24px left the rows, which inset only 16px, 8px to the left.
          Dropping the `sm:` half aligns the rows with the title (measured at
          1291px: title 305, rows 305) and returns those two blocks to the 24px
          content inset they already have in the pre-Lobe baseline
          (`28b76064`), where `SettingsCard` carried no padding of its own and
          the section title's `px-4` set the same 16px rhythm. So do not
          "finish the cleanup" on those two `sm:px-6`s: that 8px step under a
          nested panel is the baseline's nesting, not a leftover. */}
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
