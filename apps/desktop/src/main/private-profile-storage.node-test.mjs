import assert from "node:assert/strict";
import {
  mkdir,
  mkdtemp,
  rm,
  stat,
  writeFile,
} from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { after, test } from "node:test";
import { pathToFileURL } from "node:url";

const moduleUrl = pathToFileURL(
  join(import.meta.dirname, "private-profile-storage.ts"),
).href;
const { hardenExistingDesktopProfiles } = await import(moduleUrl);

const roots = [];
after(async () => {
  await Promise.all(
    roots.splice(0).map((root) => rm(root, { recursive: true, force: true })),
  );
});

test(
  "repairs every Desktop-owned profile without touching ordinary CLI profiles",
  { skip: process.platform === "win32" },
  async () => {
    const root = await mkdtemp(join(tmpdir(), "patchbay-profile-migrate-"));
    roots.push(root);
    const desktop = join(root, "desktop-api.example.com");
    const ordinary = join(root, "work");
    await mkdir(desktop, { recursive: true, mode: 0o755 });
    await mkdir(ordinary, { recursive: true, mode: 0o755 });
    for (const name of ["config.json", ".desktop-user-id", ".config.lock"]) {
      await writeFile(join(desktop, name), "private", { mode: 0o644 });
    }
    await writeFile(join(ordinary, "config.json"), "ordinary", { mode: 0o644 });

    assert.equal(await hardenExistingDesktopProfiles(root), 1);
    assert.equal((await stat(desktop)).mode & 0o777, 0o700);
    for (const name of ["config.json", ".desktop-user-id", ".config.lock"]) {
      assert.equal((await stat(join(desktop, name))).mode & 0o777, 0o600);
    }
    assert.equal((await stat(ordinary)).mode & 0o777, 0o755);
    assert.equal((await stat(join(ordinary, "config.json"))).mode & 0o777, 0o644);
  },
);

test("ignores a missing profiles root", async () => {
  const root = await mkdtemp(join(tmpdir(), "patchbay-profile-missing-"));
  roots.push(root);
  assert.equal(await hardenExistingDesktopProfiles(join(root, "profiles")), 0);
});
