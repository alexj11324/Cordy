// @vitest-environment node
import { mkdir, mkdtemp, rm, stat, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import process from "node:process";
import { afterEach, describe, expect, it } from "vitest";

import { hardenExistingDesktopProfiles } from "./private-profile-storage";

const fixtureRoots: string[] = [];

afterEach(async () => {
  await Promise.all(fixtureRoots.splice(0).map((root) => rm(root, { recursive: true, force: true })));
});

async function fixtureRoot(prefix: string): Promise<string> {
  const root = await mkdtemp(join(tmpdir(), prefix));
  fixtureRoots.push(root);
  return root;
}

describe("hardenExistingDesktopProfiles", () => {
  it.skipIf(process.platform === "win32")(
    "repairs every Desktop-owned profile without touching ordinary CLI profiles",
    async () => {
      const root = await fixtureRoot("patchbay-profile-migrate-");
      const desktop = join(root, "desktop-api.example.com");
      const ordinary = join(root, "work");
      await mkdir(desktop, { recursive: true, mode: 0o755 });
      await mkdir(ordinary, { recursive: true, mode: 0o755 });
      for (const name of ["config.json", ".desktop-user-id", ".config.lock"]) {
        await writeFile(join(desktop, name), "private", { mode: 0o644 });
      }
      await writeFile(join(ordinary, "config.json"), "ordinary", {
        mode: 0o644,
      });

      expect(await hardenExistingDesktopProfiles(root)).toBe(1);
      expect((await stat(desktop)).mode & 0o777).toBe(0o700);
      for (const name of ["config.json", ".desktop-user-id", ".config.lock"]) {
        expect((await stat(join(desktop, name))).mode & 0o777).toBe(0o600);
      }
      expect((await stat(ordinary)).mode & 0o777).toBe(0o755);
      expect((await stat(join(ordinary, "config.json"))).mode & 0o777).toBe(
        0o644,
      );
    },
  );

  it("ignores a missing profiles root", async () => {
    const root = await fixtureRoot("patchbay-profile-missing-");
    expect(await hardenExistingDesktopProfiles(join(root, "profiles"))).toBe(0);
  });
});
