# macOS desktop login preview

Use the packaged preview to validate native login and callbacks:

```sh
pnpm --filter @patchbay/desktop preview
```

Quit that checkout's existing preview first. The command prepares Go binaries
from the current source, builds the renderer/main/preload, and uses the existing
electron-builder configuration to produce a locally ad-hoc-signed application.
It installs the generated app under `~/Applications/Orvilo Development/`.
Do not run the callback acceptance test from an app bundle inside `/tmp`:
Launch Services marks those bundles `in-temp-dir` and can exclude them from
URL dispatch even when their protocol declarations and signatures are valid.

The preview has a stable, path-derived bundle ID and callback scheme for its
checkout. Its build metadata selects the corresponding existing Canary data
directory. It declares only that callback scheme, never production `patchbay`.
The launcher verifies the signature, protocol declaration and actual macOS
handler. It refuses to replace a running preview. Preview updates cannot
replace it with the production app.

A production-built preview reads the regular validated desktop configuration,
including the hosted defaults when `~/.patchbay/desktop.json` is absent.
`pnpm dev:desktop` remains the frontend development loop with development
endpoint overrides; opening that Electron window is not native-login acceptance.
Release signing/notarization and production application identity are unchanged.

## Acceptance

- Login opens the browser, completes authorization, and returns to this app.
- Repeat with the app closed while authorization is pending.
- Logout immediately returns to the Login / Guest choice; cleanup completes
  before either choice can create a new session.
- Back from browser waiting returns to the choice and invalidates that attempt.
  A late callback must not sign the user in after cancellation.
- Guest creates a server-owned anonymous session and uses the normal onboarding,
  workspace and task UI. Create a task and restart to verify persistence.
- An unreadable legacy local Guest marker exposes reset on the entry page.
  Resetting the marker does not delete the old selected directory or run history.

Rebuilding is required after merging. An old browser handoff link is not a new
login attempt and may already be cancelled, consumed or expired.

## Automatic executable discovery

Opening the app and automatic daemon discovery do not execute login/interactive
shell initialization. Discovery checks inherited PATH, explicit
`ORVILO_<PROVIDER>_PATH` settings, conventional installer directories and
existing provider-specific app locations. Candidate discovery does not execute
the candidate. Version probes and task execution are separate operations.

An installation exposed only by arbitrary shell rc logic must supply an explicit
executable path or launch with the intended PATH. The app does not run rc files
to infer it: those scripts can access protected folders, start processes, and
outlive a timeout. No TCC reset or blanket filesystem permission is required.
