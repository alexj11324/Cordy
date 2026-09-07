"use client";

import { useEffect, type ReactNode } from "react";
import { useAgentThreadPanelStore } from "@patchbay/core/agent-thread";
import { useCurrentWorkspace } from "@patchbay/core/paths";
import {
  ResizableHandle,
  ResizablePanel,
  ResizablePanelGroup,
} from "@patchbay/ui/components/ui/resizable";
import { useNavigation } from "../../navigation";
import { useT } from "../../i18n";
import { TaskAgentThreadPanel } from "./task-agent-thread-panel";

/**
 * Stable app-canvas split for task-backed Agent conversations. The main panel
 * never unmounts when the conversation opens or closes, preserving route
 * scroll, form drafts, and component state.
 */
export function AgentThreadPanelLayout({ children }: { children: ReactNode }) {
  const { t } = useT("issues");
  const workspaceId = useCurrentWorkspace()?.id ?? null;
  const { pathname } = useNavigation();
  const panel = useAgentThreadPanelStore((state) => state.panel);
  const closePanel = useAgentThreadPanelStore((state) => state.closePanel);
  const isCurrent = !!panel && panel.workspaceId === workspaceId && panel.routePath === pathname;

  useEffect(() => {
    if (panel && !isCurrent) closePanel();
  }, [closePanel, isCurrent, panel]);

  return (
    <ResizablePanelGroup
      orientation="horizontal"
      className="min-h-0 min-w-0 flex-1 overflow-hidden"
    >
      <ResizablePanel id="agent-thread-main" minSize="35%">
        <div className="flex h-full min-h-0 min-w-0 flex-col overflow-hidden">
          {children}
        </div>
      </ResizablePanel>
      {isCurrent ? (
        <>
          <ResizableHandle aria-label={t(($) => $.agent_thread.resize)} />
          <ResizablePanel
            id="agent-thread-sidebar"
            defaultSize={420}
            minSize={320}
            maxSize="65%"
            groupResizeBehavior="preserve-pixel-size"
            className="min-w-0 overflow-hidden"
          >
            <TaskAgentThreadPanel
              key={`${panel.workspaceId}:${panel.taskId}`}
              workspaceId={panel.workspaceId}
              taskId={panel.taskId}
              onClose={closePanel}
            />
          </ResizablePanel>
        </>
      ) : null}
    </ResizablePanelGroup>
  );
}
