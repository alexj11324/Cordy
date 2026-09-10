"use client";

import { useState } from "react";
import { useProjectDraftStore } from "@orvilo/core/projects";
import { Button } from "@orvilo/ui/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from "@orvilo/ui/components/ui/dialog";
import { CreateProjectModal } from "../create-project";
import { useT } from "../../i18n";
import { ProjectBuilderPanel } from "./project-builder-panel";
import type { ProjectBuilderDraft } from "./project-builder-draft";

type ProjectCreateMode = "choose" | "manual" | "agent";

/**
 * Registered entry point for project creation. It keeps the existing manual
 * modal intact while making the agent builder reachable from the same action.
 */
export function CreateProjectFlowModal({ onClose }: { onClose: () => void }) {
  const { t } = useT("modals");
  const [mode, setMode] = useState<ProjectCreateMode>("choose");
  const draft = useProjectDraftStore((state) => state.draft);
  const setDraft = useProjectDraftStore((state) => state.setDraft);

  if (mode === "manual") {
    return <CreateProjectModal onClose={onClose} />;
  }

  const currentDraft: ProjectBuilderDraft = {
    title: draft.title,
    summary: draft.summary ?? "",
    description: draft.description,
    icon: draft.icon ?? null,
    status: draft.status,
    priority: draft.priority,
    lead_type: draft.leadType ?? null,
    lead_id: draft.leadId ?? null,
    start_date: draft.startDate ?? null,
    due_date: draft.dueDate ?? null,
    member_ids: [],
    label_ids: [],
    dependency_ids: [],
  };

  const applyBuilderDraft = (proposal: ProjectBuilderDraft) => {
    setDraft({
      title: proposal.title,
      summary: proposal.summary,
      description: proposal.description,
      icon: proposal.icon ?? undefined,
      status: proposal.status,
      priority: proposal.priority,
      leadType: proposal.lead_type ?? undefined,
      leadId: proposal.lead_id ?? undefined,
      startDate: proposal.start_date ?? undefined,
      dueDate: proposal.due_date ?? undefined,
    });
    setMode("manual");
  };

  return (
    <Dialog open onOpenChange={(open) => { if (!open) onClose(); }}>
      {mode === "choose" ? (
        <DialogContent>
          <DialogHeader>
            <DialogTitle>{t(($) => $.create_project.title)}</DialogTitle>
            <DialogDescription>
              {t(($) => $.create_project.choose_method)}
            </DialogDescription>
          </DialogHeader>
          <div className="grid gap-2 sm:grid-cols-2">
            <Button variant="outline" onClick={() => setMode("manual")}>
              {t(($) => $.create_project.create_manually)}
            </Button>
            <Button onClick={() => setMode("agent")}>
              {t(($) => $.create_project.create_with_agent)}
            </Button>
          </div>
        </DialogContent>
      ) : (
        <DialogContent
          showCloseButton={false}
          className="!h-[min(48rem,calc(100dvh-2rem))] !max-w-[calc(100vw-2rem)] gap-0 overflow-hidden p-0 sm:!max-w-4xl"
        >
          <DialogTitle className="sr-only">
            {t(($) => $.create_project.create_with_agent)}
          </DialogTitle>
          <DialogDescription className="sr-only">
            {t(($) => $.create_project.builder_intro)}
          </DialogDescription>
          <ProjectBuilderPanel
            currentDraft={currentDraft}
            onApply={applyBuilderDraft}
            onClose={onClose}
            onSwitchToManual={() => setMode("manual")}
          />
        </DialogContent>
      )}
    </Dialog>
  );
}
