import { AppHeader } from "./app-header"
import { SidebarShell } from "./sidebar-shell"

export function AppShell() {
  return (
    <SidebarShell>
      {/* Header */}
      <AppHeader />
      <div className="flex flex-1 flex-col gap-4 p-4">
        <div className="grid auto-rows-min gap-4 md:grid-cols-3">
          <div className="border-border/40 bg-muted/40 aspect-video rounded-lg border border-dashed" />
          <div className="border-border/40 bg-muted/40 aspect-video rounded-lg border border-dashed" />
          <div className="border-border/40 bg-muted/40 aspect-video rounded-lg border border-dashed" />
        </div>
        <div className="border-border/40 bg-muted/40 h-full flex-1 rounded-lg border border-dashed" />
      </div>
    </SidebarShell>
  )
}