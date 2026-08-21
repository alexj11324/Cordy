import { createDraftStore } from "../drafts/create-draft-store";

interface FeedbackDraft {
  message: string;
}

export const useFeedbackDraftStore = createDraftStore<FeedbackDraft>({
  storageKey: "cordy_feedback_draft",
  emptyData: { message: "" },
  hasMeaningful: (d) => !!d.message,
});
