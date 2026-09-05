# Local card preview

From the repository root, run `pnpm dev:cards`, then open
<http://127.0.0.1:5188>. No server, database, login, Electron, or daemon is needed.
Refresh after editing shared components; HMR is intentionally disabled because
the page's CSP denies all connections (Vite's client may log a blocked socket).

The page imports the production `BoardCardContent`, desktop stylesheet/fonts,
and real query keys. It does not copy card markup, animations, or state derivation.
Change the card in `packages/views/issues/components/board-card.tsx`, or its shared
styles, and both the preview and application use that change at the same revision.
Different deployed revisions can still look different until deployed.

Issue status (`in_progress` / `in_review`) and execution (`idle` / `queued` /
`running`) are separate controls. Running supplies an in-memory `AgentTask`
snapshot; it does not create a task. The existing Working text shimmers; this
also exercises the restored running border beam from the shared production card.

The preview has its own in-memory QueryClient and view store. Query fetching is
paused, API methods fail locally, and CSP blocks fetch, XHR, WebSocket and beacon
connections. There is no auth initialization or execution provider. Navigation
is inert. Fixtures must never contain real credentials, IDs or external assets.

This is a separate loopback-only dev entry, not a route in web, desktop, or
staging. Its Vite config rejects builds, and production entrypoints do not import
it. It verifies display states only; staging must still test real state changes,
permissions, events and Agent execution before release.

Checks:

- `pnpm --filter @patchbay/desktop typecheck:cards`
- `pnpm test:cards` (install Playwright Chromium first, or use
  `PLAYWRIGHT_CHANNEL=chrome pnpm test:cards` with local Chrome)
- `pnpm --filter @patchbay/desktop exec vite build --config card-preview/vite.config.ts`
  must fail with the development-only error.
