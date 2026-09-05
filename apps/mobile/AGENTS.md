# Mobile App Rules

These rules apply under `apps/mobile/`. Repository-wide package and state
ownership rules remain in the root `AGENTS.md`.

## Product parity

Mobile may use native interaction patterns, but product semantics must match the
web and desktop clients:

- Preserve server-visible counts, enums, permissions, transitions, and
  cross-cache side effects.
- Before implementing a feature whose behavior already exists elsewhere,
  inspect the relevant `packages/core/<feature>/` and
  `packages/views/<feature>/` code. Look specifically for display transforms
  such as deduplication, coalescing, filtering, and grouping.
- Reuse pure functions and types from `@patchbay/core`. Mobile owns
  platform-specific rendering, navigation, query keys, and cache shapes.
- Document intentional UI divergence at the implementation boundary so a
  maintainer can find the shared semantic source.

## UI decisions

Use the first suitable option:

1. An existing shipped mobile pattern.
2. A public iOS or Expo native component.
3. A React Native Reusables component using its default behavior.
4. A small composition local to the feature.

Create a shared primitive only when at least three current callers need it.
When none of these choices can satisfy a materially new interaction, explain
the tradeoff before introducing a custom component.

Theme colors come from `global.css` and semantic NativeWind utilities. New UI
must work in light and dark modes. Expo Router `formSheet` routes own long
lists, search, and forms; compact confirmations may use a dialog.

The detailed component inventory and migration status live in
`docs/rnr-migration.md`; read the relevant section only when changing shared
UI primitives or migration-owned components.

## Data and realtime

- React Query owns server state. Zustand owns mobile view state.
- Every workspace-scoped domain defines one local query-key factory. Mutations,
  reconnect handling, and realtime updates use that factory rather than inline
  key arrays.
- Use the mobile API validation helpers for typed response bodies. Forward
  TanStack Query's abort signal, preserve the hard request timeout, and keep
  authentication failures on the shared unauthorized path.
- Patch a cache when an event contains the complete updated object. Invalidate
  only when the event lacks enough data to construct the authoritative result.
- Mount workspace-list subscriptions for the workspace session and record-level
  subscriptions on the owning screen. Do not mount listeners for data the
  mobile UI does not consume.
- A realtime event is authoritative over an older optimistic value. Patch all
  affected domain caches, including cross-feature surfaces such as inbox rows.
- Optimistic UI that must be visible before navigation sets predictable cache
  state synchronously before awaiting cancellation. Unpredictable create/delete
  results wait for the server.

Event types are defined in `@patchbay/core/types/events.ts`. Inspect the
matching web updater for semantic coverage, then implement against mobile's own
cache shapes.

## Build and dependencies

- Route local iOS runs through `scripts/ios-run.sh`; it keeps Expo prebuild and
  runtime configuration aligned.
- Use `pnpm exec expo install` for Expo-aligned packages. For other packages,
  inspect current dist-tags and peer requirements before choosing a version.
- When adding a source directory, run `git check-ignore -v` on a representative
  file and confirm the committed files with `git ls-files`.
- Add libraries only after the codebase has adopted them. The current baseline
  and migration plan are documented in `docs/rnr-migration.md`.

## Verification

Scale verification to the affected behavior:

- Run focused type, lint, and unit checks for the changed domain.
- For navigation, layout, native interaction, signing, or realtime behavior,
  build and exercise the real mobile path.
- Realtime changes require a second-client check showing that the mobile screen
  updates without manual refresh.
- A build or static source inspection does not complete a user-visible mobile
  change when the runtime path remains untested. Report the missing check
  explicitly.
