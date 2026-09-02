// @vitest-environment node
import { chmod, mkdtemp, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import process from "node:process";
import { afterEach, describe, expect, it } from "vitest";

import {
  DESKTOP_PROFILE_HELPER_ARG,
  runDesktopProfileHelper,
} from "./desktop-profile-helper";

const fixtureRoots: string[] = [];

afterEach(async () => {
  await Promise.all(fixtureRoots.splice(0).map((root) => rm(root, { recursive: true, force: true })));
});

async function fixture(contents: string): Promise<string> {
  const root = await mkdtemp(join(tmpdir(), "patchbay-profile-helper-"));
  fixtureRoots.push(root);
  const executable = join(root, "helper");
  await writeFile(executable, `#!/usr/bin/env node\n${contents}\n`, {
    mode: 0o700,
  });
  await chmod(executable, 0o700);
  return executable;
}

describe.skipIf(process.platform === "win32")("runDesktopProfileHelper", () => {
  it("sends credentials only over stdin with the fixed private argv", async () => {
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

  it("rejects a failed helper without echoing the request token", async () => {
    const executable = await fixture(`
process.stdin.resume();
process.stdin.on("end", () => {
  process.stderr.write("deliberate helper failure");
  process.exit(7);
});`);

    let failure: unknown;
    try {
      await runDesktopProfileHelper(
        executable,
        {
          action: "set_credentials",
          profile: "desktop-api.example.com",
          server_url: "https://api.example.com",
          token: "must-not-appear",
          user_id: "user-1",
        },
        process.env,
      );
    } catch (error) {
      failure = error;
    }
    expect(failure).toBeInstanceOf(Error);
    expect((failure as Error).message).toMatch(/exit 7.*deliberate helper failure/);
    expect((failure as Error).message).not.toContain("must-not-appear");
  });

  it("waits for process close before reporting a timeout", async () => {
    const executable = await fixture(`
process.on("SIGTERM", () => {});
process.stdin.resume();
process.stdin.on("end", () => setTimeout(() => process.exit(9), 500));`);
    const started = Date.now();

    await expect(
      runDesktopProfileHelper(
        executable,
        { action: "clear_credentials", profile: "desktop-api.example.com" },
        process.env,
        200,
      ),
    ).rejects.toThrow(/timed out/);
    expect(Date.now() - started).toBeGreaterThanOrEqual(400);
  });
});
