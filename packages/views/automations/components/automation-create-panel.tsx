"use client";

import { useId, useState } from "react";
import { ArrowLeft, CalendarClock, Rocket } from "lucide-react";
import { toast } from "sonner";
import { useWorkspaceId } from "@patchbay/core/hooks";
import {
  useCreateAutomation,
  useCreateAutomationTrigger,
} from "@patchbay/core/automations/mutations";
import type {
  AutomationAssigneeType,
  AutomationExecutionMode,
} from "@patchbay/core/types";
import { Button } from "@patchbay/ui/components/ui/button";
import {
  Card,
  CardContent,
  CardDescription,
  CardFooter,
  CardHeader,
  CardTitle,
} from "@patchbay/ui/components/ui/card";
import {
  Field,
  FieldDescription,
  FieldError,
  FieldGroup,
  FieldLabel,
} from "@patchbay/ui/components/ui/field";
import { Input } from "@patchbay/ui/components/ui/input";
import {
  Select,
  SelectContent,
  SelectGroup,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@patchbay/ui/components/ui/select";
import { Textarea } from "@patchbay/ui/components/ui/textarea";
import { ProjectPicker } from "../../projects/components/project-picker";
import { useT } from "../../i18n";
import { AgentPicker, type AssigneeSelection } from "./pickers/agent-picker";
import { SubscriberMultiSelect } from "./subscriber-multi-select";
import { ScheduleEditor } from "./schedule-editor/schedule-editor";
import {
  getDefaultScheduleConfig,
  type ScheduleConfig,
} from "./schedule-editor/model";
import { browserTimezone } from "../../common/timezone-select";
import { toCron } from "./schedule-editor/cron-mapping";
import { useScheduleSubmitGate } from "./schedule-editor/validate";
import { formatSchedulePartialFailureToast } from "./automation-dialog-toast";

export interface AutomationCreateSeed {
  title?: string;
  description?: string;
  executionMode?: AutomationExecutionMode;
  schedule?: Pick<ScheduleConfig, "time" | "days">;
  preset?: string | null;
}

export function AutomationCreatePanel({
  seed,
  onCancel,
  onCreated,
}: {
  seed: AutomationCreateSeed;
  onCancel: () => void;
  onCreated: (automationId: string) => void;
}) {
  const { t } = useT("automations");
  const wsId = useWorkspaceId();
  const createAutomation = useCreateAutomation();
  const createTrigger = useCreateAutomationTrigger();
  const scheduleGate = useScheduleSubmitGate(wsId);
  const fallbackSchedule = getDefaultScheduleConfig(browserTimezone());
  const [title, setTitle] = useState(seed.title ?? "");
  const [description, setDescription] = useState(seed.description ?? "");
  const [assignee, setAssignee] = useState<AssigneeSelection | null>(null);
  const [executionMode, setExecutionMode] = useState<AutomationExecutionMode>(
    seed.executionMode ?? "create_issue",
  );
  const [projectId, setProjectId] = useState<string | null>(null);
  const [subscriberUserIds, setSubscriberUserIds] = useState<string[]>([]);
  const [schedule, setSchedule] = useState<ScheduleConfig>(() =>
    seed.schedule
      ? { ...fallbackSchedule, ...seed.schedule }
      : fallbackSchedule,
  );
  const [showErrors, setShowErrors] = useState(false);
  const [submitting, setSubmitting] = useState(false);
  const titleErrorId = useId();
  const assigneeErrorId = useId();
  const hasSchedule = Boolean(seed.schedule);
  const hasPreset = Boolean(seed.preset);
  const modeItems = [
    {
      value: "create_issue" as const,
      label: t(($) => $.dialog.output_modes.create_issue.label),
    },
    {
      value: "run_only" as const,
      label: t(($) => $.dialog.output_modes.run_only.label),
    },
  ];

  const handleSubmit = async (event: React.FormEvent) => {
    event.preventDefault();
    if (submitting) return;
    if (!title.trim() || !assignee) {
      setShowErrors(true);
      return;
    }
    setSubmitting(true);
    try {
      if (hasSchedule && !(await scheduleGate.ensureAccepted(schedule))) return;
      const automation = await createAutomation.mutateAsync({
        title: title.trim(),
        description: description.trim() || undefined,
        project_id: projectId,
        executor_type: assignee.type as AutomationAssigneeType,
        executor_id: assignee.id,
        execution_mode: executionMode,
        subscribers:
          executionMode === "create_issue"
            ? subscriberUserIds.map((user_id) => ({
                user_type: "member" as const,
                user_id,
              }))
            : [],
      });

      let triggerError: string | null = null;
      try {
        if (hasSchedule) {
          await createTrigger.mutateAsync({
            automationId: automation.id,
            kind: "schedule",
            cron_expression: toCron(schedule),
            timezone: schedule.timezone,
          });
        } else if (seed.preset) {
          await createTrigger.mutateAsync({
            automationId: automation.id,
            kind: "webhook",
            preset: seed.preset,
            ...(seed.preset === "github.workflow_run.completed" ||
            seed.preset === "github.ci_completed"
              ? { config: { on_failure: true } }
              : {}),
          });
        }
      } catch (error) {
        triggerError =
          error instanceof Error && error.message ? error.message : "unknown";
      }

      if (triggerError) {
        toast.error(
          formatSchedulePartialFailureToast(t, "create", triggerError),
        );
      } else {
        toast.success(t(($) => $.dialog.toast_created));
      }
      onCreated(automation.id);
    } catch (error) {
      toast.error(
        error instanceof Error && error.message
          ? error.message
          : t(($) => $.dialog.toast_create_failed),
      );
    } finally {
      setSubmitting(false);
    }
  };

  return (
    <div className="min-h-0 flex-1 overflow-y-auto px-4 py-6 sm:px-6">
      <form
        onSubmit={handleSubmit}
        className="mx-auto w-full max-w-4xl"
        data-testid="automation-create-panel"
      >
        <Card>
          <CardHeader>
            <CardTitle>{t(($) => $.create_panel.title)}</CardTitle>
            <CardDescription>
              {t(($) => $.create_panel.description)}
            </CardDescription>
          </CardHeader>
          <CardContent>
            <FieldGroup className="grid gap-5 lg:grid-cols-[minmax(0,1fr)_320px]">
              <div className="space-y-5">
                <Field data-invalid={showErrors && !title.trim()}>
                  <FieldLabel htmlFor="automation-create-title">
                    {t(($) => $.create_panel.name)}
                  </FieldLabel>
                  <Input
                    id="automation-create-title"
                    autoFocus
                    value={title}
                    onChange={(event) => setTitle(event.target.value)}
                    aria-describedby={
                      showErrors && !title.trim() ? titleErrorId : undefined
                    }
                    aria-invalid={showErrors && !title.trim()}
                    placeholder={t(($) => $.dialog.title_placeholder)}
                  />
                  {showErrors && !title.trim() ? (
                    <FieldError id={titleErrorId}>
                      {t(($) => $.dialog.error_title_required)}
                    </FieldError>
                  ) : null}
                </Field>

                <Field>
                  <FieldLabel htmlFor="automation-create-runbook">
                    {t(($) => $.dialog.runbook_label)}
                  </FieldLabel>
                  <FieldDescription>
                    {t(($) => $.dialog.runbook_hint)}
                  </FieldDescription>
                  <Textarea
                    id="automation-create-runbook"
                    value={description}
                    onChange={(event) => setDescription(event.target.value)}
                    placeholder={t(($) => $.dialog.description_placeholder)}
                    className="min-h-64 resize-y font-mono text-body leading-6"
                  />
                </Field>
              </div>

              <div className="space-y-5">
                <Field data-invalid={showErrors && !assignee}>
                  <FieldLabel>{t(($) => $.dialog.section_assignee)}</FieldLabel>
                  <AgentPicker
                    assignee={assignee}
                    onChange={setAssignee}
                    disabled={submitting}
                    triggerRender={
                      <Button
                        type="button"
                        variant="outline"
                        className="w-full justify-start"
                        aria-describedby={
                          showErrors && !assignee ? assigneeErrorId : undefined
                        }
                        aria-invalid={showErrors && !assignee}
                      />
                    }
                  />
                  {showErrors && !assignee ? (
                    <FieldError id={assigneeErrorId}>
                      {t(($) => $.dialog.error_assignee_required)}
                    </FieldError>
                  ) : null}
                </Field>

                <Field>
                  <FieldLabel>
                    {t(($) => $.dialog.section_output_mode)}
                  </FieldLabel>
                  <Select
                    items={modeItems}
                    value={executionMode}
                    onValueChange={(value) => {
                      if (value) setExecutionMode(value);
                    }}
                    disabled={submitting}
                  >
                    <SelectTrigger
                      className="w-full"
                      aria-label={t(($) => $.dialog.section_output_mode)}
                    >
                      <SelectValue />
                    </SelectTrigger>
                    <SelectContent align="start">
                      <SelectGroup>
                        {modeItems.map((item) => (
                          <SelectItem key={item.value} value={item.value}>
                            {item.label}
                          </SelectItem>
                        ))}
                      </SelectGroup>
                    </SelectContent>
                  </Select>
                </Field>

                <Field>
                  <FieldLabel>{t(($) => $.dialog.section_project)}</FieldLabel>
                  <FieldDescription>
                    {t(($) => $.dialog.project_hint)}
                  </FieldDescription>
                  <ProjectPicker
                    projectId={projectId}
                    disabled={submitting}
                    onUpdate={(updates) =>
                      setProjectId(updates.project_id ?? null)
                    }
                    triggerRender={
                      <Button
                        type="button"
                        variant="outline"
                        className="w-full justify-start"
                      />
                    }
                  />
                </Field>

                {executionMode === "create_issue" ? (
                  <Field>
                    <FieldLabel>
                      {t(($) => $.dialog.section_subscribers)}
                    </FieldLabel>
                    <SubscriberMultiSelect
                      selectedIds={subscriberUserIds}
                      onChange={setSubscriberUserIds}
                    />
                  </Field>
                ) : null}

                {hasSchedule ? (
                  <Field>
                    <FieldLabel>
                      {t(($) => $.dialog.section_schedule)}
                    </FieldLabel>
                    <ScheduleEditor
                      value={schedule}
                      onChange={setSchedule}
                      wsId={wsId}
                      disabled={submitting}
                    />
                  </Field>
                ) : hasPreset ? (
                  <div className="flex items-center gap-2 rounded-lg border bg-muted/30 px-3 py-2 text-caption text-muted-foreground">
                    <CalendarClock className="size-4" aria-hidden="true" />
                    {t(($) => $.create_panel.template_trigger)}
                  </div>
                ) : null}
              </div>
            </FieldGroup>
          </CardContent>
          <CardFooter className="justify-between">
            <Button type="button" variant="ghost" onClick={onCancel}>
              <ArrowLeft aria-hidden="true" />
              {t(($) => $.create_panel.back)}
            </Button>
            <Button type="submit" disabled={submitting} aria-busy={submitting}>
              <Rocket aria-hidden="true" />
              {submitting
                ? t(($) => $.dialog.creating)
                : t(($) => $.page.new_automation)}
            </Button>
          </CardFooter>
        </Card>
      </form>
    </div>
  );
}
