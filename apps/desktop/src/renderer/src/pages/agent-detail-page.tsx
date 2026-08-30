import { useParams } from "react-router-dom";
import { useQuery } from "@tanstack/react-query";
import { AgentDetailPage as SharedAgentDetailPage } from "@patchbay/views/agents";
import { useWorkspaceId } from "@patchbay/core/hooks";
import { agentListOptions } from "@patchbay/core/workspace/queries";
import { useDocumentTitle } from "@/hooks/use-document-title";
import { isDesktopWebPreview } from "../platform/web-bridge";

export function AgentDetailPage() {
  const { id } = useParams<{ id: string }>();
  const wsId = useWorkspaceId();
  const { data: agents = [] } = useQuery(agentListOptions(wsId));
  const agent = agents.find((a) => a.id === id) ?? null;
  const isPreview = isDesktopWebPreview();

  useDocumentTitle(agent?.name ?? "Agent");

  if (!id) return null;
  return <SharedAgentDetailPage agentId={id} readOnly={isPreview} />;
}
