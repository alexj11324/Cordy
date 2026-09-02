// @vitest-environment node

import { mkdir, mkdtemp, readFile, rm, stat, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, describe, expect, it } from "vitest";
import {
  clearLocalGuestSession,
  loadLocalGuestSession,
  localGuestSessionPath,
  saveLocalGuestSession,
} from "./local-guest-session-storage";

const temporaryDirectories: string[] = [];

afterEach(async () => {
  await Promise.all(
    temporaryDirectories.splice(0).map((directory) =>
      rm(directory, { recursive: true, force: true }),
    ),
  );
});

async function createSessionPath(): Promise<{
  directory: string;
  filePath: string;
}> {
  const directory = await mkdtemp(join(tmpdir(), "patchbay-guest-"));
  temporaryDirectories.push(directory);
  return { directory, filePath: localGuestSessionPath(directory) };
}

describe("local Guest session storage", () => {
  it("writes atomically with private file and directory permissions", async () => {
    const { directory, filePath } = await createSessionPath();

    await saveLocalGuestSession(filePath, { displayName: "Alice" });

    const guestDirectory = join(directory, "local-guest");
    expect((await stat(guestDirectory)).mode & 0o777).toBe(0o700);
    expect((await stat(filePath)).mode & 0o777).toBe(0o600);
    expect(JSON.parse(await readFile(filePath, "utf8"))).toEqual({
      displayName: "Alice",
    });
    await expect(loadLocalGuestSession(filePath)).resolves.toEqual({
      ok: true,
      session: { displayName: "Alice" },
    });
  });

  it("does not persist invalid names and reports corrupted state", async () => {
    const { filePath } = await createSessionPath();

    await expect(
      saveLocalGuestSession(filePath, { displayName: " Alice" }),
    ).rejects.toThrow("invalid local guest display name");
    await mkdir(join(filePath, ".."), { recursive: true });
    await writeFile(filePath, '{"displayName":" Alice"}\n', {
      mode: 0o600,
    });
    await expect(loadLocalGuestSession(filePath)).resolves.toEqual({
      ok: false,
      reason: "invalid",
    });
  });

  it("clears a session without affecting the parent user data directory", async () => {
    const { directory, filePath } = await createSessionPath();
    await saveLocalGuestSession(filePath, { displayName: "Alice" });

    await clearLocalGuestSession(filePath);

    await expect(loadLocalGuestSession(filePath)).resolves.toEqual({
      ok: true,
      session: null,
    });
    await expect(stat(directory)).resolves.toBeTruthy();
  });
});
