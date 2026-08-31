#!/usr/bin/env bash
# Build and run the iOS app, re-applying app.config.ts to ios/ first.
#
# `expo run:ios` only prebuilds when ios/ is missing (ensureNativeProjectAsync
# in @expo/cli): when the directory already exists it returns early and config
# plugins are never re-applied. Everything driven by app.config.ts — app icon,
# bundle identifier, display name, URL scheme, Info.plist permission strings —
# then stays frozen at whatever the first prebuild produced, while the build
# still reports success.
#
# ios/ is gitignored and fully generated, so prebuilding on every run is safe
# and idempotent. --no-install is fine because run:ios installs pods itself
# when Podfile.lock is out of date. --clean is deliberately avoided in the
# normal edit/run loop so CocoaPods and Xcode can reuse native intermediates;
# use a clean prebuild only after changing native dependencies or config plugins.
#
# APP_ENV and the .env file are supplied by the calling package.json script, so
# prebuild and run resolve the same variant. Arguments are forwarded to
# run:ios only — prebuild takes the same flags for every variant.
set -euo pipefail

node "$(dirname "$0")/disable-ios-source-build.mjs"
pnpm exec expo prebuild -p ios --no-install
exec pnpm exec expo run:ios "$@"
