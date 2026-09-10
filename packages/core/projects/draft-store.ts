import type { ProjectStatus, ProjectPriority } from "../types";
import { createDraftStore } from "../drafts/create-draft-store";

interface ProjectDraft {
  title: string;
  summary: string;
  description: string;
  status: ProjectStatus;
  priority: ProjectPriority;
  leadType?: "member" | "agent";
  leadId?: string;
  icon?: string;
  // Calendar days ("YYYY-MM-DD"); empty/undefined means unset.
  startDate?: string;
  dueDate?: string;
}

const EMPTY_DRAFT: ProjectDraft = {
  title: "",
  summary: "",
  description: "",
  status: "planned",
  priority: "none",
  leadType: undefined,
  leadId: undefined,
  icon: undefined,
  startDate: undefined,
  dueDate: undefined,
};

export const useProjectDraftStore = createDraftStore<ProjectDraft>({
  storageKey: "orvilo_project_draft",
  emptyData: EMPTY_DRAFT,
  hasMeaningful: (d) => !!(d.title || d.summary || d.description),
});
