"use client";

import { useEffect, type CSSProperties } from "react";
import { PanelRight } from "lucide-react";
import { useChatStore } from "@orvilo/core/chat";
import { useAgentThreadPanelStore } from "@orvilo/core/agent-thread";
import { useCurrentWorkspace, useWorkspacePaths } from "@orvilo/core/paths";
import { Button } from "@orvilo/ui/components/ui/button";
import { useNavigation } from "../navigation";
import { useT } from "../i18n";
import { ChatWindow } from "./components/chat-window";
import { TaskAgentThreadPanel } from "../agent-thread/components/task-agent-thread-panel";

/** The shell owns this control, so route actions cannot hide its entry point. */
export function GlobalRightSidebarToggle({ inSidebar = false }: { inSidebar?: boolean }) {
  const { t } = useT("chat");
  const isOpen = useChatStore((state) => state.isOpen);
  const toggle = useChatStore((state) => state.toggle);
  if (inSidebar !== isOpen) return null;
  const label = isOpen ? t(($) => $.sidebar.close) : t(($) => $.sidebar.open);
  return (
    <Button
      variant="ghost"
      size="icon-sm"
      className="shrink-0"
      aria-label={label}
      title={label}
      aria-expanded={isOpen}
      aria-controls="global-right-sidebar"
      onClick={toggle}
      style={{ WebkitAppRegion: "no-drag" } as CSSProperties}
    >
      <PanelRight />
    </Button>
  );
}

/** In-flow shell column. The existing core store persists its open state. */
export function GlobalRightSidebar() {
  const { t } = useT("chat");
  const isOpen = useChatStore((state) => state.isOpen);
  const agentPanel = useAgentThreadPanelStore((state) => state.panel);
  const closeAgentPanel = useAgentThreadPanelStore((state) => state.closePanel);
  const workspace = useCurrentWorkspace();
  const { pathname } = useNavigation();
  const paths = useWorkspacePaths();
  const chatPath = paths.chat();
  const isChatPage = pathname === chatPath || pathname.startsWith(`${chatPath}/`);
  const isCurrentAgentPanel =
    agentPanel !== null &&
    agentPanel.workspaceId === workspace?.id &&
    agentPanel.routePath === pathname;

  useEffect(() => {
    if (agentPanel && !isCurrentAgentPanel) closeAgentPanel();
  }, [agentPanel, closeAgentPanel, isCurrentAgentPanel]);

  const visible = isOpen || isCurrentAgentPanel;
  return (
    <aside
      id="global-right-sidebar"
      aria-label={t(($) => $.sidebar.title)}
      hidden={!visible}
      className={visible
        ? "flex h-full min-h-0 w-[420px] max-w-[65%] shrink-0 flex-col overflow-hidden border-l border-border/60 bg-background"
        : "hidden"}
    >
      {isCurrentAgentPanel ? (
        <TaskAgentThreadPanel
          workspaceId={agentPanel.workspaceId}
          taskId={agentPanel.taskId}
          onClose={closeAgentPanel}
          closeIcon="panel"
        />
      ) : isChatPage ? (
        <>
          <header
            className="flex h-12 shrink-0 items-center justify-end gap-2 px-3"
            style={{ WebkitAppRegion: "drag" } as CSSProperties}
          >
            <GlobalRightSidebarToggle inSidebar />
          </header>
          <p className="p-4 text-body text-muted-foreground">{t(($) => $.sidebar.chat_page_notice)}</p>
        </>
      ) : (
        <ChatWindow docked headerEnd={<GlobalRightSidebarToggle inSidebar />} />
      )}
    </aside>
  );
}
