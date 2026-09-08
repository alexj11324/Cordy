# Orvilo production brand

Approved symbol: solid monochrome, two opposing arches, two-unit seams.
`mark.svg` is the transparent vector master. `Orvilo-A.icon` contains two
independently editable SVG layers. Foreground glass, highlights and shadows
are disabled; the platform background uses white/default and black/dark.

`desktop-1024.png` and `ios-1024.png` were exported with Icon Composer MCP's
Apple renderer. Run `node docs/assets/brand/orvilo/generate-app-icons.mjs` to
regenerate desktop PNG sizes, ICO and ICNS from the macOS render. The shipped
copies live in apps/desktop/build and apps/desktop/resources. Web launcher
sizes derive from the desktop render; maskable and touch icons derive from
apps/web/public/icons/icon.svg. Mobile's icon.png is an opaque iOS export.

Display names, app chrome, and workspace packages (`@orvilo/*`,
`github.com/orvilo-ai/orvilo/server`) use Orvilo. `ORVILO_*` is the sole
environment-variable namespace; the old `ORVILO_*` names are not read.

Existing installs still depend on historical machine identity: app IDs,
`orvilo://` callback schemes, storage keys, desktop data directories,
`X-Orvilo-*` API headers, the `orvilo` CLI binary and artifact names,
credentials, and service origins. This keeps sessions and provider connections
stable while the visible brand and internal packages use Orvilo.
