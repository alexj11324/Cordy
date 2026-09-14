import { LobeThemeBridge } from "../lobe";
import { IntegrationsTab } from "../settings/components/integrations-tab";

/**
 * The workspace-wide integrations page, served by
 * `apps/web/app/[workspaceSlug]/(dashboard)/integrations/page.tsx`.
 *
 * **The bridge belongs here, at the surface root.** `IntegrationsTab`'s channel
 * dialog renders `SlackTab` / `TelegramTab`, which are Lobe since Task 7a, and
 * the dialog sits outside the `standalone ? … : …` ternary — so it renders on
 * every host of this component, not only under the settings dialog. Without a
 * bridge above it, a Lobe `Button` throws
 * `Please wrap your app with <ConfigProvider> (or <MotionProvider>)` from
 * `useMotionComponent` the moment a workspace with a connected channel opens
 * this page.
 *
 * It is mounted here rather than inside `SlackTab`: the settings page, the chat
 * message list and the feedback editor all mount this bridge at their own
 * roots, and a bridge *inside* a shared component would nest a `ThemeProvider`
 * whose semantics nobody has measured. One decision per surface, no nesting.
 *
 * The agent pane is the third host of an integrations view, and it is not this
 * page's to wrap: it renders
 * `packages/views/agents/components/tabs/integrations-tab.tsx`, and that
 * surface mounts its own bridge at its own root
 * (`agents/components/agent-detail-page.tsx:323`). Its `*AgentBindButton` family
 * is still on the shadcn primitives, but that is a conversion this round did not
 * take rather than a missing bridge. See `reference-lobe-tab-migration.md`,
 * "the bridge is a HOST-SURFACE constraint".
 */
export function WorkspaceIntegrationsPage() {
  return (
    // No `skin`: this page renders `IntegrationsTab` standalone — the settings
    // design system without the dialog. There is one token set app-wide, so it
    // cannot diverge from the dialog.
    <LobeThemeBridge>
      <IntegrationsTab standalone />
    </LobeThemeBridge>
  );
}
