import assert from "node:assert/strict";
import { chmod, mkdtemp, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { after, test } from "node:test";
import { pathToFileURL } from "node:url";

const moduleUrl = pathToFileURL(
  join(import.meta.dirname, "desktop-profile-helper.ts"),
).href;
const { DESKTOP_PROFILE_HELPER_ARG, runDesktopProfileHelper } =
  await import(moduleUrl);

const roots = [];
after(async () => {
  await Promise.all(
    roots.splice(0).map((root) => rm(root, { recursive: true, force: true })),
  );
});

async function fixture(contents) {
  const root = await mkdtemp(join(tmpdir(), "patchbay-profile-helper-"));
  roots.push(root);
  const executable = join(root, "helper");
  await writeFile(executable, `#!/usr/bin/env node\n${contents}\n`, {
    mode: 0o700,
  });
  await chmod(executable, 0o700);
  return executable;
}

test("sends credentials only over stdin with the fixed private argv", async () => {
  const executable = await fixture(`
let input = "";
process.stdin.setEncoding("utf8");
process.stdin.on("data", (chunk) => { input += chunk; });
process.stdin.on("end", () => {
  const request = JSON.parse(input);
  if (process.argv[2] !== ${JSON.stringify(DESKTOP_PROFILE_HELPER_ARG)}) process.exit(2);
  if (process.argv.length !== 3) process.exit(3);
  if (request.action !== "set_credentials" || request.token !== "fixture-token" || request.user_id !== "user-1") process.exit(4);
});`);

  await runDesktopProfileHelper(
    executable,
    {
      action: "set_credentials",
      profile: "desktop-api.example.com",
      server_url: "https://api.example.com",
      token: "fixture-token",
      user_id: "user-1",
    },
    process.env,
  );
});

test("rejects a failed helper without echoing the request token", async () => {
  const executable = await fixture(`
process.stdin.resume();
process.stdin.on("end", () => {
  process.stderr.write("deliberate helper failure");
  process.exit(7);
});`);

  await assert.rejects(
    runDesktopProfileHelper(
      executable,
      {
        action: "set_credentials",
        profile: "desktop-api.example.com",
        server_url: "https://api.example.com",
        token: "must-not-appear",
        user_id: "user-1",
      },
      process.env,
    ),
    (error) => {
      assert.match(error.message, /exit 7.*deliberate helper failure/);
      assert.doesNotMatch(error.message, /must-not-appear/);
      return true;
    },
  );
});

test("timeout waits for the helper process to close before rejecting", async () => {
  const executable = await fixture(`
process.on("SIGTERM", () => {});
process.stdin.resume();
process.stdin.on("end", () => setTimeout(() => process.exit(9), 500));`);
  const started = Date.now();

  await assert.rejects(
    runDesktopProfileHelper(
      executable,
      { action: "clear_credentials", profile: "desktop-api.example.com" },
      process.env,
      200,
    ),
    /timed out/,
  );
  assert.ok(Date.now() - started >= 400);
});
