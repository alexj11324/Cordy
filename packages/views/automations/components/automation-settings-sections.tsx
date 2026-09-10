"use client";

import type { ReactNode, Ref } from "react";
import type { AutomationTriggerPreset } from "@orvilo/core/automations";
import {
  ContentEditor,
  ReadonlyContent,
  type ContentEditorRef,
} from "../../editor";
import { useT } from "../../i18n";
import { AgentPicker, type AssigneeSelection } from "./pickers/agent-picker";
import { TriggerAddMenu } from "./trigger-add-menu";

export function AutomationTriggersSection({
  children,
  canWrite,
  onPickSchedule,
  onPickPreset,
}: {
  children?: ReactNode;
  canWrite: boolean;
  onPickSchedule: () => void;
  onPickPreset: (preset: AutomationTriggerPreset) => void;
}) {
  const { t } = useT("automations");
  return (
    <section className="space-y-2">
      <h2 className="text-caption font-medium uppercase tracking-wider text-muted-foreground">
        {t(($) => $.detail.section_triggers)}
      </h2>
      <div
        data-testid="automation-triggers-card"
        className="divide-y rounded-lg border bg-background"
      >
        {children}
        <div className="px-2 py-1">
          <TriggerAddMenu
            canWrite={canWrite}
            variant="inset"
            onPickSchedule={onPickSchedule}
            onPickPreset={onPickPreset}
          />
        </div>
      </div>
    </section>
  );
}

export function AutomationInstructionsCard({
  description,
  assignee,
  canWrite,
  busy = false,
  onDescriptionChange,
  onAssigneeChange,
  debounceMs = 1200,
  editorRef,
}: {
  description: string;
  assignee: AssigneeSelection | null;
  canWrite: boolean;
  busy?: boolean;
  onDescriptionChange: (description: string) => void;
  onAssigneeChange: (assignee: AssigneeSelection) => void;
  debounceMs?: number;
  editorRef?: Ref<ContentEditorRef>;
}) {
  const { t } = useT("automations");
  return (
    <section className="space-y-2" data-testid="automation-instructions">
      <h2 className="text-caption font-medium uppercase tracking-wider text-muted-foreground">
        {t(($) => $.settings.section_instructions)}
      </h2>
      <div className="rounded-lg border bg-background">
        <div className="automation-instructions h-44 overflow-y-auto overscroll-contain px-4 py-3">
          {canWrite ? (
            <ContentEditor
              ref={editorRef}
              value={description}
              placeholder={t(($) => $.dialog.description_placeholder)}
              onUpdate={onDescriptionChange}
              debounceMs={debounceMs}
              flushPendingOnUnmount
              showBubbleMenu={false}
            />
          ) : description ? (
            <ReadonlyContent content={description} />
          ) : (
            <p className="text-label text-muted-foreground">
              {t(($) => $.dialog.description_placeholder)}
            </p>
          )}
        </div>
        <div className="flex min-w-0 items-center px-3 pb-2">
          <AgentPicker
            assignee={assignee}
            disabled={!canWrite || busy}
            onChange={onAssigneeChange}
          />
        </div>
      </div>
    </section>
  );
}
