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

The brand changes display names, not machine identity. Existing package names,
app IDs, callback schemes, storage keys, desktop data directories, API headers,
artifact names, credentials and service origins retain their Patchbay identity.
This avoids disconnecting existing users during a visual rebrand.
