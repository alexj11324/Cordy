import { useParams } from "react-router-dom";
import { useQuery } from "@tanstack/react-query";
import { AutomationDetailPage as AutomationDetail } from "@orvilo/views/automations/components";
import { useWorkspaceId } from "@orvilo/core/hooks";
import { automationDetailOptions } from "@orvilo/core/automations/queries";
import { useDocumentTitle } from "@/hooks/use-document-title";

export function AutomationDetailPage() {
  const { id } = useParams<{ id: string }>();
  const wsId = useWorkspaceId();
  const { data } = useQuery(automationDetailOptions(wsId, id!));

  // Plain text only — no leading ⚡ glyph in the title (MUL-4370).
  useDocumentTitle(data ? data.automation.title : "Automation");

  if (!id) return null;
  return <AutomationDetail automationId={id} />;
}
