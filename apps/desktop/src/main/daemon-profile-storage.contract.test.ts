// @vitest-environment node
import { readFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";

const source = readFileSync(
  resolve(dirname(fileURLToPath(import.meta.url)), "daemon-manager.ts"),
  "utf8",
);

describe("Desktop profile storage wiring", () => {
  it("routes profile mutations through the private Go helper", () => {
    expect(source).toContain('from "./desktop-profile-helper"');
    expect(source).toContain('action: "configure"');
    expect(source).toContain('action: "set_credentials"');
    expect(source).toContain('action: "clear_credentials"');
    expect(source).toContain("await runDesktopProfileHelper(");
    expect(source).not.toContain("async function writeProfileConfig(");
    expect(source).not.toContain("async function writeProfileUserId(");
  });

  it("serializes complete Electron credential transitions", () => {
    expect(source).toContain("let profileMutationChain: Promise<void>");
    expect(source).toContain("function serializeProfileMutation<T>");

    const setup = source.slice(source.indexOf("export function setupDaemonManager"));
    expect(setup).toContain("return serializeProfileMutation(async () => {");
    expect(setup).toContain("serializeProfileMutation(() => clearToken())");
    expect(setup).toContain(
      "serializeProfileMutation(() => reauthenticate(token, userId))",
    );
  });

  it("hardens existing profiles and strips task-scoped config from children", () => {
    expect(source).toContain("hardenExistingDesktopProfiles(profilesRoot)");
    expect(source).toContain("delete env.PATCHBAY_TASK_CONFIG_ROOT");
    expect(source).toContain("await configureDesktopProfile(bin, active)");
    expect(source).toContain(
      '{ timeout: 15_000, env: desktopSpawnEnv() }',
    );
  });
});
