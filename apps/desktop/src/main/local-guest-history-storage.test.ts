// @vitest-environment node

import { mkdtemp, readFile, rm, stat, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { afterEach, describe, expect, it } from "vitest";
import { MAX_LOCAL_GUEST_HISTORY_ENTRIES } from "../shared/local-guest";
import {
  clearLocalGuestRunHistory,
  loadLocalGuestRunHistory,
  localGuestRunHistoryPath,
  saveLocalGuestRunHistory,
} from "./local-guest-history-storage";

const temporaryDirectories: string[] = [];

afterEach(async () => {
  await Promise.all(
    temporaryDirectories
      .splice(0)
      .map((directory) => rm(directory, { recursive: true, force: true })),
  );
});

async function createHistoryPath(): Promise<string> {
  const directory = await mkdtemp(join(tmpdir(), "patchbay-guest-history-"));
  temporaryDirectories.push(directory);
  return localGuestRunHistoryPath(directory);
}

const RUN = {
  id: "9a1b",
  prompt: "inspect the workspace",
  workingDirectory: "/home/user/project",
  status: "completed",
  output: "3 files, 1 directories, 42 bytes",
  startedAt: 1_700_000_000_000,
  durationMs: 120,
};

describe("local Guest run history storage", () => {
  it("keeps the history private to the user account", async () => {
    // Guest history records what the user asked and which directories they
    // opened. It is local-only data with no server copy, so the on-disk
    // permissions are the only thing protecting it on a shared machine.
    const filePath = await createHistoryPath();

    await saveLocalGuestRunHistory(filePath, {
      lastDirectory: RUN.workingDirectory,
      runs: [RUN],
    });

    const directoryStat = await stat(dirname(filePath));
    const fileStat = await stat(filePath);
    expect(directoryStat.mode & 0o777).toBe(0o700);
    expect(fileStat.mode & 0o777).toBe(0o600);
  });

  it("round-trips a saved history", async () => {
    const filePath = await createHistoryPath();
    const history = { lastDirectory: RUN.workingDirectory, runs: [RUN] };

    await saveLocalGuestRunHistory(filePath, history);

    await expect(loadLocalGuestRunHistory(filePath)).resolves.toEqual({
      ok: true,
      history,
    });
  });

  it("reports an empty history rather than failing on first launch", async () => {
    const filePath = await createHistoryPath();

    await expect(loadLocalGuestRunHistory(filePath)).resolves.toEqual({
      ok: true,
      history: { runs: [] },
    });
  });

  it("fails closed on corrupt or tampered state instead of repairing it", async () => {
    const filePath = await createHistoryPath();
    await saveLocalGuestRunHistory(filePath, { runs: [] });

    for (const contents of [
      "not json",
      JSON.stringify({ runs: [{ ...RUN, token: "secret" }] }),
      JSON.stringify({ runs: [{ ...RUN, startedAt: "yesterday" }] }),
      JSON.stringify({
        runs: Array.from(
          { length: MAX_LOCAL_GUEST_HISTORY_ENTRIES + 1 },
          (_value, index) => ({ ...RUN, id: `run-${index}` }),
        ),
      }),
    ]) {
      await writeFile(filePath, contents, { mode: 0o600 });
      await expect(loadLocalGuestRunHistory(filePath)).resolves.toEqual({
        ok: false,
      });
    }
  });

  it("clears the history and is safe to call when there is nothing to clear", async () => {
    const filePath = await createHistoryPath();
    await saveLocalGuestRunHistory(filePath, { runs: [RUN] });

    await clearLocalGuestRunHistory(filePath);
    await clearLocalGuestRunHistory(filePath);

    await expect(readFile(filePath, "utf8")).rejects.toMatchObject({
      code: "ENOENT",
    });
    // The parent directory survives; only the Guest data is gone.
    await expect(stat(dirname(filePath))).resolves.toBeDefined();
  });
});
