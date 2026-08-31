"use client";

import { Network } from "lucide-react";
import { useT } from "../i18n";
import { PageHeader } from "../layout/page-header";
import { DependencyGraphView } from "../issues/components/dependency-graph-view";

/**
 * Workspace-level task graph page.
 *
 * The graph canvas remains shared with the Issues surface, but this route is
 * the canonical entry point for inspecting all active dependency plans in a
 * workspace. Keeping the page chrome here prevents the sidebar destination
 * from inheriting issue-list filters or a project scope by accident.
 */
export function TaskGraphPage() {
  const { t } = useT("issues");

  return (
    <main className="flex min-h-0 flex-1 flex-col">
      <PageHeader>
        <Network className="size-4 text-muted-foreground" aria-hidden="true" />
        <h1 className="text-body font-medium">{t(($) => $.graph.title)}</h1>
      </PageHeader>
      <div className="flex min-h-0 flex-1 flex-col">
        <DependencyGraphView />
      </div>
    </main>
  );
}
