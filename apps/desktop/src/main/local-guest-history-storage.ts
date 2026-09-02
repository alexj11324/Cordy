import { chmod, mkdir, open, readFile, rename, unlink } from "node:fs/promises";
import { randomUUID } from "node:crypto";
import { dirname, join } from "node:path";
import {
  parseLocalGuestRunHistory,
  type LocalGuestRunHistory,
} from "../shared/local-guest";

const HISTORY_DIRECTORY = "local-guest";
const HISTORY_FILE = "history.json";

export function localGuestRunHistoryPath(userDataPath: string): string {
  return join(userDataPath, HISTORY_DIRECTORY, HISTORY_FILE);
}

export type LocalGuestHistoryReadResult =
  | { ok: true; history: LocalGuestRunHistory }
  | { ok: false };

export async function loadLocalGuestRunHistory(
  filePath: string,
): Promise<LocalGuestHistoryReadResult> {
  try {
    const raw = await readFile(filePath, "utf8");
    const parsed = parseLocalGuestRunHistory(JSON.parse(raw) as unknown);
    return parsed ? { ok: true, history: parsed } : { ok: false };
  } catch (error) {
    if ((error as NodeJS.ErrnoException).code === "ENOENT") {
      return { ok: true, history: { runs: [] } };
    }
    return { ok: false };
  }
}

export async function saveLocalGuestRunHistory(
  filePath: string,
  history: LocalGuestRunHistory,
): Promise<void> {
  const directory = dirname(filePath);
  await mkdir(directory, { recursive: true, mode: 0o700 });
  await chmod(directory, 0o700);

  const temporaryPath = `${filePath}.${randomUUID()}.tmp`;
  const handle = await open(temporaryPath, "wx", 0o600);
  try {
    await handle.writeFile(JSON.stringify(history), "utf8");
    await handle.sync();
    await chmod(temporaryPath, 0o600);
  } finally {
    await handle.close();
  }
  await rename(temporaryPath, filePath);
  await chmod(filePath, 0o600);
}

export async function clearLocalGuestRunHistory(filePath: string): Promise<void> {
  try {
    await unlink(filePath);
  } catch (error) {
    if ((error as NodeJS.ErrnoException).code !== "ENOENT") throw error;
  }
}
