"use client";

import { useRef, useState } from "react";
import { Loader2, Plus, Trash2 } from "lucide-react";
import { toast } from "sonner";
import {
  automationTriggerPreset,
  type AutomationToolsConfig,
  type AutomationTriggerPreset,
} from "@orvilo/core/automations";
import {
  useCreateAutomation,
  useCreateAutomationTrigger,
} from "@orvilo/core/automations/mutations";
import { useAuthStore } from "@orvilo/core/auth";
import { AutomationSettingsPage } from "./automation-settings-page";
import { useWorkspaceId } from "@orvilo/core/hooks";
import { useWorkspacePaths } from "@orvilo/core/paths";
import { Button } from "@orvilo/ui/components/ui/button";
import { useNavigation } from "../../navigation";
import type { ContentEditorRef } from "../../editor";
import { browserTimezone } from "../../common/timezone-select";
import { useT } from "../../i18n";
import type { AssigneeSelection } from "./pickers/agent-picker";
import {
  AutomationInstructionsCard,
  AutomationTriggersSection,
} from "./automation-settings-sections";
import { AutomationToolsSection } from "./automation-tools-section";
import { AutomationTriggerSourceGlyph } from "./trigger-add-menu";
import { TriggerScheduleDialog } from "./trigger-schedule-dialog";
import {
  AUTOMATION_TEMPLATES,
  templateTriggerPreset,
  type AutomationTemplate,
} from "./automation-templates";
import {
  getDefaultScheduleConfig,
  type ScheduleConfig,
} from "./schedule-editor/model";
import { describeSchedule } from "./schedule-editor/describe";
import { toCron } from "./schedule-editor/cron-mapping";
import { useScheduleSubmitGate } from "./schedule-editor/validate";
import { formatSchedulePartialFailureToast } from "./automation-dialog-toast";
import { DraftTriggerConnection } from "./draft-trigger-connection";

type DraftTrigger =
  | { kind: "schedule"; schedule: ScheduleConfig }
  | { kind: "webhook"; preset: AutomationTriggerPreset };

function templateFromId(id: string | null): AutomationTemplate | null {
  return id && Object.prototype.hasOwnProperty.call(AUTOMATION_TEMPLATES, id)
    ? AUTOMATION_TEMPLATES[id as keyof typeof AUTOMATION_TEMPLATES]
    : null;
}

export function AutomationCreateSettingsPage() {
  const { t } = useT("automations");
  const navigation = useNavigation();
  const paths = useWorkspacePaths();
  const wsId = useWorkspaceId();
  const currentUser = useAuthStore((state) => state.user);
  const [template] = useState(() =>
    templateFromId(navigation.searchParams.get("template")),
  );
  const [title, setTitle] = useState(() =>
    template ? t(($) => $.templates[template.id].title) : "",
  );
  const [description, setDescription] = useState(template?.prompt ?? "");
  const [assignee, setAssignee] = useState<AssigneeSelection | null>(null);
  const [projectId, setProjectId] = useState<string | null>(null);
  const [tools, setTools] = useState<AutomationToolsConfig>({});
  const [triggers, setTriggers] = useState<DraftTrigger[]>(() => {
    if (!template) return [];
    if (template.triggerKind === "schedule")
      return [
        {
          kind: "schedule",
          schedule: {
            ...getDefaultScheduleConfig(browserTimezone()),
            ...template.schedule,
          },
        },
      ];
    const preset = automationTriggerPreset(
      templateTriggerPreset(template.trigger),
    );
    return preset ? [{ kind: "webhook", preset }] : [];
  });
  const [scheduleIndex, setScheduleIndex] = useState<number | null>(null);
  const [showErrors, setShowErrors] = useState(false);
  const [saving, setSaving] = useState(false);
  const submitting = useRef(false);
  const editor = useRef<ContentEditorRef>(null);
  const createAutomation = useCreateAutomation();
  const createTrigger = useCreateAutomationTrigger();
  const gate = useScheduleSubmitGate(wsId);

  const submit = async () => {
    if (submitting.current) return;
    if (!title.trim() || !assignee) {
      setShowErrors(true);
      return;
    }
    const currentDescription = editor.current?.getMarkdown() ?? description;
    submitting.current = true;
    setSaving(true);
    try {
      for (const trigger of triggers) {
        if (
          trigger.kind === "schedule" &&
          !(await gate.ensureAccepted(trigger.schedule))
        )
          return;
      }
      const automation = await createAutomation.mutateAsync({
        title: title.trim(),
        description: currentDescription.trim() || undefined,
        executor_type: assignee.type,
        executor_id: assignee.id,
        project_id: projectId,
        execution_mode: "create_issue",
        tools: { ...tools },
        subscribers: [],
      });
      const errors: string[] = [];
      for (const trigger of triggers) {
        try {
          await createTrigger.mutateAsync(
            trigger.kind === "schedule"
              ? {
                  automationId: automation.id,
                  kind: "schedule",
                  cron_expression: toCron(trigger.schedule),
                  timezone: trigger.schedule.timezone,
                }
              : {
                  automationId: automation.id,
                  kind: "webhook",
                  preset: trigger.preset.id,
                  ...(trigger.preset.id === "github.workflow_run.completed" ||
                  trigger.preset.id === "github.ci_completed"
                    ? { config: { on_failure: true } }
                    : {}),
                },
          );
        } catch (error) {
          errors.push(error instanceof Error ? error.message : String(error));
        }
      }
      if (errors.length)
        toast.error(
          formatSchedulePartialFailureToast(t, "create", errors.join(" · ")),
        );
      else toast.success(t(($) => $.dialog.toast_created));
      navigation.replace(paths.automationDetail(automation.id));
    } catch (error) {
      toast.error(
        error instanceof Error
          ? error.message
          : t(($) => $.dialog.toast_create_failed),
      );
    } finally {
      submitting.current = false;
      setSaving(false);
    }
  };

  const selectedSchedule =
    scheduleIndex !== null ? triggers[scheduleIndex] : undefined;
  return (
    <div
      className="flex min-h-0 flex-1 flex-col"
      data-testid="automation-create-settings"
    >
      <AutomationSettingsPage
        title={title}
        onTitleChange={setTitle}
        titleError={
          showErrors && !title.trim()
            ? t(($) => $.dialog.error_title_required)
            : undefined
        }
        status="draft"
        creatorName={currentUser?.name ?? ""}
        projectId={projectId}
        onProjectChange={setProjectId}
        canWrite={!saving}
        busy={saving}
        tab="settings"
        actions={
          <Button size="sm" disabled={saving} onClick={() => void submit()}>
            {saving ? <Loader2 className="animate-spin" /> : <Plus />}
            {saving
              ? t(($) => $.dialog.saving)
              : t(($) => $.page.new_automation)}
          </Button>
        }
      >
        <AutomationTriggersSection
          canWrite={!saving}
          onPickSchedule={() => setScheduleIndex(triggers.length)}
          onPickPreset={(preset) =>
            setTriggers((current) => [...current, { kind: "webhook", preset }])
          }
        >
          {triggers.map((trigger, index) => (
            <div key={index} className="flex flex-wrap items-center gap-3 px-4 py-3">
              <AutomationTriggerSourceGlyph
                source={
                  trigger.kind === "schedule"
                    ? "scheduled"
                    : trigger.preset.source
                }
              />
              <div className="min-w-0 flex-1">
                {trigger.kind === "schedule" ? (
                  <Button
                    variant="ghost"
                    size="sm"
                    className="h-auto max-w-full whitespace-normal px-0 text-left"
                    disabled={saving}
                    onClick={() => setScheduleIndex(index)}
                  >
                    {describeSchedule(t, trigger.schedule) ??
                      toCron(trigger.schedule)}{" "}
                    · {trigger.schedule.timezone}
                  </Button>
                ) : (
                  <>
                    <p className="text-body">
                      {t(
                        ($) =>
                          $.presets[
                            trigger.preset.labelKey as keyof typeof $.presets
                          ],
                      )}
                    </p>
                    <p className="mt-1 text-caption text-muted-foreground">
                      {t(($) => $.create_settings.provider_after_save)}
                    </p>
                  </>
                )}
              </div>
              {trigger.kind === "webhook" && (
                <DraftTriggerConnection
                  provider={trigger.preset.provider}
                  disabled={saving}
                />
              )}
              <Button
                variant="ghost"
                size="icon-sm"
                className="shrink-0"
                disabled={saving}
                aria-label={t(($) => $.create_settings.remove_trigger)}
                onClick={() =>
                  setTriggers((current) =>
                    current.filter((_, item) => item !== index),
                  )
                }
              >
                <Trash2 />
              </Button>
            </div>
          ))}
        </AutomationTriggersSection>
        <div>
          <AutomationInstructionsCard
            description={description}
            assignee={assignee}
            canWrite={!saving}
            busy={saving}
            onDescriptionChange={setDescription}
            onAssigneeChange={setAssignee}
            debounceMs={0}
            editorRef={editor}
          />
          {showErrors && !assignee ? (
            <p role="alert" className="mt-2 text-caption text-destructive">
              {t(($) => $.dialog.error_assignee_required)}
            </p>
          ) : null}
        </div>
        <AutomationToolsSection
          automation={{ id: "", tools: { ...tools } }}
          canWrite={!saving}
          saving={saving}
          onToolsChange={setTools}
        />
      </AutomationSettingsPage>
      {scheduleIndex !== null ? (
        <TriggerScheduleDialog
          automationId=""
          initialSchedule={
            selectedSchedule?.kind === "schedule"
              ? selectedSchedule.schedule
              : undefined
          }
          onSaveDraft={(schedule) =>
            setTriggers((current) => {
              const next = [...current];
              next[scheduleIndex] = { kind: "schedule", schedule };
              return next;
            })
          }
          onOpenChange={(open) => {
            if (!open) setScheduleIndex(null);
          }}
        />
      ) : null}
    </div>
  );
}
