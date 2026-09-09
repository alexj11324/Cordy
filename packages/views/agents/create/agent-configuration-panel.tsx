"use client";

import { useEffect, useRef, type ReactNode } from "react";
import {
  applyDraftModelChange,
  applyDraftRuntimeChange,
  type AgentDraft,
  type AgentPermissionScope,
} from "@orvilo/core/agents";
import { isRuntimeUsableForUser } from "@orvilo/core/runtimes";
import { useConfigStore } from "@orvilo/core/config";
import type { MemberWithUser, RuntimeDevice } from "@orvilo/core/types";
import { Checkbox } from "@orvilo/ui/components/ui/checkbox";
import { Input } from "@orvilo/ui/components/ui/input";
import { cn } from "@orvilo/ui/lib/utils";
import { ActorAvatar } from "../../common/actor-avatar";
import { useT } from "../../i18n";
import {
  SettingsCard,
  SettingsRow,
  SettingsSection,
} from "../../settings/components/settings-layout";
import type { ModelSelection } from "../components/model-selector-content";
import { ModelDropdown } from "../components/model-dropdown";
import { ConversationStartersEditor } from "../components/conversation-starters-editor";

const PERMISSION_SCOPES: AgentPermissionScope[] = [
  "private",
  "workspace",
  "members",
];

export function AgentConfigurationPanel({
  draft,
  onChange,
  runtimes,
  runtimesLoading,
  members,
  currentUserId,
  nameError,
  onNameChange,
  compact = false,
  showConversationStarters = true,
  onModelSelection,
  runtimeSwitchPending = false,
  runtimeSwitchInFlight = false,
}: {
  draft: AgentDraft;
  onChange: (draft: AgentDraft) => void;
  runtimes: RuntimeDevice[];
  runtimesLoading: boolean;
  members: MemberWithUser[];
  currentUserId: string | null;
  nameError: string | null;
  onNameChange: (name: string) => void;
  compact?: boolean;
  /** Existing builder sessions retain their editor; new plain agents omit it. */
  showConversationStarters?: boolean;
  /** Builder sessions rebind the server-side carrier instead of only editing
   *  the draft. Absent for the plain create flows, where the draft is the only
   *  state that exists. */
  onModelSelection?: (selection: ModelSelection) => Promise<void>;
  /** A builder reply is in flight, so the server would refuse the rebind. */
  runtimeSwitchPending?: boolean;
  /** A rebind request is in flight. */
  runtimeSwitchInFlight?: boolean;
}) {
  const { t } = useT("agents");
  const conversationStartersSupported = useConfigStore(
    (state) => state.agentConversationStartersSupported,
  );
  const selectedRuntime =
    runtimes.find((runtime) => runtime.id === draft.runtimeId) ?? null;
  const set = <K extends keyof AgentDraft>(key: K, value: AgentDraft[K]) =>
    onChange({ ...draft, [key]: value });
  const otherMembers = members.filter(
    (member) => member.user_id !== currentUserId,
  );
  const runtimeLocked = runtimeSwitchPending || runtimeSwitchInFlight;

  return (
    <div className={cn("space-y-8", compact && "space-y-6")}>
      <SettingsSection
        title={t(($) => $.creation_studio.sections.identity)}
        description={t(($) => $.creation_studio.sections.identity_hint)}
      >
        <SettingsCard>
          <AgentNameField
            compact={compact}
            name={draft.name}
            error={nameError}
            onChange={onNameChange}
          />
        </SettingsCard>
        {showConversationStarters && conversationStartersSupported ? (
          <SettingsCard>
            <div className="px-4 py-4">
              <ConversationStartersEditor
                value={draft.conversationStarters}
                onChange={(value) => set("conversationStarters", value)}
              />
            </div>
          </SettingsCard>
        ) : null}
      </SettingsSection>

      <SettingsSection
        title={t(($) => $.creation_studio.sections.execution)}
        description={t(($) => $.creation_studio.sections.execution_hint)}
      >
        <SettingsCard>
          <SettingsRow
            label={t(($) => $.model_dropdown.label)}
            size="none"
          >
          <ModelDropdown
            showLabel={false}
            variant="chip"
            runtimeId={selectedRuntime?.id ?? null}
            runtimeOnline={selectedRuntime?.status === "online"}
            value={draft.model}
            thinkingLevel={draft.thinkingLevel}
            serviceTier={draft.serviceTier}
            provider={selectedRuntime?.provider}
            runtimes={runtimes.filter((runtime) =>
              isRuntimeUsableForUser(runtime, currentUserId),
            )}
            onSelection={(selection) => {
              if (onModelSelection) return onModelSelection(selection);
              const { runtimeId, model, thinkingLevel, serviceTier } = selection;
              return onChange({
                ...applyDraftModelChange(
                  runtimeId === draft.runtimeId
                    ? draft
                    : applyDraftRuntimeChange(draft, runtimeId),
                  model,
                ),
                thinkingLevel,
                serviceTier,
              });
            }}
            onChange={(value) => onChange(applyDraftModelChange(draft, value))}
            // A successful switch clears the model, so an edit made while the
            // rebind is in flight would be silently discarded.
            disabled={runtimesLoading || runtimeLocked}
          />
          </SettingsRow>
          {runtimeSwitchPending && (
            <p className="px-4 pb-3 text-caption text-muted-foreground">
              {t(($) => $.creation_studio.builder.switch_runtime_pending)}
            </p>
          )}
        </SettingsCard>
      </SettingsSection>

      <SettingsSection
        title={t(($) => $.creation_studio.sections.access)}
      >
        <SettingsCard>
          <div
            className="space-y-1 p-2"
            role="radiogroup"
            aria-label={t(($) => $.creation_studio.sections.access)}
          >
            {PERMISSION_SCOPES.map((scope) => (
              <button
                key={scope}
                type="button"
                role="radio"
                aria-checked={draft.permissionScope === scope}
                onClick={() => set("permissionScope", scope)}
                className={cn(
                  "flex w-full items-center gap-3 rounded-md px-3 py-2.5 text-left transition-colors",
                  "hover:bg-muted focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring",
                  draft.permissionScope === scope && "bg-muted",
                )}
              >
                <span
                  className={cn(
                    "flex size-3.5 shrink-0 items-center justify-center rounded-full border",
                    draft.permissionScope === scope && "border-primary",
                  )}
                  aria-hidden="true"
                >
                  {draft.permissionScope === scope ? (
                    <span className="size-1.5 rounded-full bg-primary" />
                  ) : null}
                </span>
                <span className="min-w-0 text-body font-medium">
                  {t(($) => $.creation_studio.access[scope].title)}
                </span>
              </button>
            ))}
          </div>
          {draft.permissionScope === "members" ? (
            <div className="max-h-48 overflow-y-auto p-2">
              {otherMembers.map((member) => {
                const checked = draft.memberIds.has(member.user_id);
                return (
                  <label
                    key={member.user_id}
                    className="flex cursor-pointer items-center gap-2 rounded-md px-2 py-2 hover:bg-muted"
                  >
                    <Checkbox
                      checked={checked}
                      onCheckedChange={(value) => {
                        const next = new Set(draft.memberIds);
                        if (value === true) next.add(member.user_id);
                        else next.delete(member.user_id);
                        set("memberIds", next);
                      }}
                    />
                    <ActorAvatar
                      actorType="member"
                      actorId={member.user_id}
                      size="sm"
                    />
                    <span className="min-w-0 flex-1 truncate text-body">
                      {member.name}
                    </span>
                  </label>
                );
              })}
              {draft.memberIds.size === 0 ? (
                <p className="px-2 py-1 text-caption text-destructive">
                  {t(($) => $.creation_studio.access.members.required)}
                </p>
              ) : null}
            </div>
          ) : null}
        </SettingsCard>
      </SettingsSection>
    </div>
  );
}

export function AgentNameField({
  name,
  error,
  onChange,
  compact = false,
}: {
  name: string;
  error: string | null;
  onChange: (name: string) => void;
  compact?: boolean;
}) {
  const { t } = useT("agents");
  const errorId = "agent-create-name-error";
  const inputRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    if (!error) return;
    inputRef.current?.focus();
    inputRef.current?.select();
  }, [error]);

  return (
    <DraftFieldRow
      compact={compact}
      label={t(($) => $.create_dialog.name_label)}
      htmlFor="agent-create-name"
    >
      <div className={cn("space-y-1.5", !compact && "ml-auto w-full max-w-60")}>
        <Input
          ref={inputRef}
          id="agent-create-name"
          name="agent-name"
          autoComplete="off"
          aria-label={t(($) => $.create_dialog.name_label)}
          aria-invalid={!!error}
          aria-describedby={error ? errorId : undefined}
          value={name}
          onChange={(event) => onChange(event.target.value)}
          placeholder={t(($) => $.create_dialog.name_placeholder)}
        />
        {error ? (
          <p id={errorId} className="text-caption text-destructive">
            {error}
          </p>
        ) : null}
      </div>
    </DraftFieldRow>
  );
}

function DraftFieldRow({
  label,
  children,
  compact = false,
  align = "center",
  htmlFor,
}: {
  label: string;
  children: ReactNode;
  compact?: boolean;
  align?: "center" | "start";
  htmlFor?: string;
}) {
  return (
    <div
      className={cn(
        "gap-3 px-4 py-4",
        compact
          ? "flex flex-col"
          : "grid sm:grid-cols-[minmax(0,1fr)_minmax(280px,1.2fr)] sm:gap-8",
        !compact && (align === "center" ? "sm:items-center" : "sm:items-start"),
      )}
    >
      {htmlFor ? (
        <label htmlFor={htmlFor} className="text-body font-medium">
          {label}
        </label>
      ) : (
        <div className="text-body font-medium">{label}</div>
      )}
      <div className="min-w-0">{children}</div>
    </div>
  );
}
