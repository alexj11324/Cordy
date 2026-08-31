#!/usr/bin/env bash
# Tests for scripts/ios-run.sh.
#
# The wrapper exists so that `expo prebuild` always runs before `expo run:ios`
# (run:ios skips prebuild whenever ios/ already exists, freezing everything
# app.config.ts owns). These tests stub pnpm on PATH and assert the call
# sequence, so they need no node_modules, no Xcode, and no device.
set -eu

SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
TEST_DIR=$(mktemp -d "${TMPDIR:-/tmp}/patchbay-ios-run.XXXXXX")
BIN_DIR="$TEST_DIR/bin"
CALLS_FILE="$TEST_DIR/pnpm-calls.log"
PROPERTIES_FILE="$TEST_DIR/Podfile.properties.json"
FIXTURE_MOBILE_DIR="$TEST_DIR/mobile-fixture"
REAL_NODE=$(command -v node)

cleanup() {
  rm -rf "$TEST_DIR"
}
trap cleanup EXIT

mkdir -p "$BIN_DIR" "$FIXTURE_MOBILE_DIR/scripts" "$FIXTURE_MOBILE_DIR/ios"
export PATCHBAY_TEST_PNPM_CALLS="$CALLS_FILE"
export PATCHBAY_TEST_REAL_NODE="$REAL_NODE"

cp "$SCRIPT_DIR/ios-run.sh" "$SCRIPT_DIR/disable-ios-source-build.mjs" \
  "$FIXTURE_MOBILE_DIR/scripts/"

# Stub pnpm: record every invocation, and optionally fail the prebuild so the
# abort-before-run case can be exercised.
cat >"$BIN_DIR/pnpm" <<'EOF'
#!/usr/bin/env bash
set -eu

printf '%s\n' "$*" >>"$PATCHBAY_TEST_PNPM_CALLS"

case "$*" in
  *prebuild*)
    if [ -n "${PATCHBAY_TEST_PROPERTIES_FILE:-}" ] &&
      grep -q 'ios.buildReactNativeFromSource' "$PATCHBAY_TEST_PROPERTIES_FILE"; then
      echo "legacy source-build property reached prebuild" >&2
      exit 1
    fi
    if [ -n "${PATCHBAY_TEST_FAIL_PREBUILD:-}" ]; then
      echo "stub prebuild failure" >&2
      exit 1
    fi
    ;;
esac
EOF
chmod +x "$BIN_DIR/pnpm"

# Keep the real Node runtime for the migration while recording its position in
# the wrapper call sequence alongside the stubbed pnpm commands.
cat >"$BIN_DIR/node" <<'EOF'
#!/usr/bin/env bash
set -eu

printf 'node %s\n' "$*" >>"$PATCHBAY_TEST_PNPM_CALLS"
exec "$PATCHBAY_TEST_REAL_NODE" "$@"
EOF
chmod +x "$BIN_DIR/node"

PATH="$BIN_DIR:$PATH"
export PATH

fail() {
  echo "FAIL: $1" >&2
  echo "--- recorded calls ---" >&2
  cat "$CALLS_FILE" >&2 || true
  exit 1
}

file_identity() {
  "$REAL_NODE" -e '
    const { statSync } = require("node:fs");
    const stat = statSync(process.argv[1], { bigint: true });
    process.stdout.write(`${stat.ino}:${stat.mtimeNs}`);
  ' "$1"
}

# --- stale source-build property is removed without disturbing other data ---
cat >"$PROPERTIES_FILE" <<'EOF'
{
  "expo.jsEngine": "hermes",
  "ios.deploymentTarget": "15.1",
  "ios.buildReactNativeFromSource": "true",
  "custom": {
    "nested": true
  }
}
EOF

node "$SCRIPT_DIR/disable-ios-source-build.mjs" "$PROPERTIES_FILE"
node - "$PROPERTIES_FILE" <<'EOF'
const fs = require("node:fs");
const properties = JSON.parse(fs.readFileSync(process.argv[2], "utf8"));

if ("ios.buildReactNativeFromSource" in properties) {
  throw new Error("legacy source-build property should be removed");
}
if (properties["expo.jsEngine"] !== "hermes") {
  throw new Error("top-level properties must be preserved");
}
if (properties["ios.deploymentTarget"] !== "15.1") {
  throw new Error("other iOS properties must be preserved");
}
if (properties.custom?.nested !== true) {
  throw new Error("nested properties must be preserved");
}
EOF

# A second migration and an already-disabled value are strict no-ops.
cp "$PROPERTIES_FILE" "$TEST_DIR/expected-properties.json"
before_identity=$(file_identity "$PROPERTIES_FILE")
node "$SCRIPT_DIR/disable-ios-source-build.mjs" "$PROPERTIES_FILE"
cmp "$PROPERTIES_FILE" "$TEST_DIR/expected-properties.json" ||
  fail "a second migration should not rewrite the properties file"
[ "$(file_identity "$PROPERTIES_FILE")" = "$before_identity" ] ||
  fail "a second migration should leave file identity and mtime unchanged"

cat >"$PROPERTIES_FILE" <<'EOF'
{
  "ios.buildReactNativeFromSource": "false",
  "ios.deploymentTarget": "15.1"
}
EOF
cp "$PROPERTIES_FILE" "$TEST_DIR/expected-properties.json"
before_identity=$(file_identity "$PROPERTIES_FILE")
node "$SCRIPT_DIR/disable-ios-source-build.mjs" "$PROPERTIES_FILE"
cmp "$PROPERTIES_FILE" "$TEST_DIR/expected-properties.json" ||
  fail "an already-disabled source-build property should not be rewritten"
[ "$(file_identity "$PROPERTIES_FILE")" = "$before_identity" ] ||
  fail "an already-disabled property should leave file identity and mtime unchanged"

# A missing generated project is safe before the first prebuild.
node "$SCRIPT_DIR/disable-ios-source-build.mjs" "$TEST_DIR/missing/Podfile.properties.json"

# --- prebuild runs before run:ios ------------------------------------------
cat >"$FIXTURE_MOBILE_DIR/ios/Podfile.properties.json" <<'EOF'
{
  "expo.jsEngine": "hermes",
  "ios.buildReactNativeFromSource": true,
  "ios.deploymentTarget": "15.1"
}
EOF
export PATCHBAY_TEST_PROPERTIES_FILE="$FIXTURE_MOBILE_DIR/ios/Podfile.properties.json"

: >"$CALLS_FILE"
"$FIXTURE_MOBILE_DIR/scripts/ios-run.sh"

expected_prebuild='exec expo prebuild -p ios --no-install'
case "$(sed -n '1p' "$CALLS_FILE")" in
  *disable-ios-source-build.mjs) ;;
  *) fail "first call should migrate legacy iOS properties" ;;
esac
[ "$(sed -n '2p' "$CALLS_FILE")" = "$expected_prebuild" ] ||
  fail "second call should be the prebuild, got: $(sed -n '2p' "$CALLS_FILE")"
[ "$(sed -n '3p' "$CALLS_FILE")" = 'exec expo run:ios' ] ||
  fail "third call should be run:ios, got: $(sed -n '3p' "$CALLS_FILE")"
[ "$(wc -l <"$CALLS_FILE")" -eq 3 ] || fail "expected exactly 3 calls"

node - "$FIXTURE_MOBILE_DIR/ios/Podfile.properties.json" <<'EOF'
const fs = require("node:fs");
const properties = JSON.parse(fs.readFileSync(process.argv[2], "utf8"));

if ("ios.buildReactNativeFromSource" in properties) {
  throw new Error("wrapper should migrate boolean true before prebuild");
}
if (properties["expo.jsEngine"] !== "hermes") {
  throw new Error("wrapper migration must preserve top-level properties");
}
if (properties["ios.deploymentTarget"] !== "15.1") {
  throw new Error("wrapper migration must preserve other iOS properties");
}
EOF

# --- the default Expo precompiled React Native path stays enabled -----------
if grep -q 'buildReactNativeFromSource' "$SCRIPT_DIR/../app.config.ts"; then
  fail "app.config.ts must not force React Native source compilation"
fi

# --- arguments forward to run:ios only --------------------------------------
: >"$CALLS_FILE"
"$FIXTURE_MOBILE_DIR/scripts/ios-run.sh" --device --configuration Release

[ "$(sed -n '2p' "$CALLS_FILE")" = "$expected_prebuild" ] ||
  fail "prebuild must not receive run:ios arguments"
[ "$(sed -n '3p' "$CALLS_FILE")" = 'exec expo run:ios --device --configuration Release' ] ||
  fail "run:ios should receive the forwarded arguments"

# --- a failed prebuild aborts before run:ios --------------------------------
: >"$CALLS_FILE"
set +e
PATCHBAY_TEST_FAIL_PREBUILD=1 "$FIXTURE_MOBILE_DIR/scripts/ios-run.sh" >/dev/null 2>&1
status=$?
set -e

[ "$status" -ne 0 ] || fail "a failed prebuild should make the wrapper exit non-zero"
grep -q 'run:ios' "$CALLS_FILE" &&
  fail "run:ios must not run after a failed prebuild"

echo "ios-run.test.sh: all assertions passed"
