import { randomUUID } from "node:crypto";
import { chmod, mkdir, open, readFile, rename, rm } from "node:fs/promises";
import { dirname, join } from "node:path";
import {
  normalizeGuestDisplayName,
  parseLocalGuestSession,
  type GuestSessionReadResult,
  type LocalGuestSession,
} from "../shared/local-guest";

export function localGuestSessionPath(userDataPath: string): string {
  return join(userDataPath, "local-guest", "session.json");
}

export async function loadLocalGuestSession(
  filePath: string,
): Promise<GuestSessionReadResult> {
  let raw: string;
  try {
    raw = await readFile(filePath, "utf8");
  } catch (err) {
    if ((err as NodeJS.ErrnoException).code === "ENOENT") {
      return { ok: true, session: null };
    }
    return { ok: false, reason: "unavailable" };
  }

  try {
    const session = parseLocalGuestSession(JSON.parse(raw) as unknown);
    return session
      ? { ok: true, session }
      : { ok: false, reason: "invalid" };
  } catch {
    return { ok: false, reason: "invalid" };
  }
}

export async function saveLocalGuestSession(
  filePath: string,
  session: LocalGuestSession,
): Promise<void> {
  if (normalizeGuestDisplayName(session.displayName) !== session.displayName) {
    throw new Error("invalid local guest display name");
  }

  const directory = dirname(filePath);
  await mkdir(directory, { recursive: true, mode: 0o700 });
  await chmod(directory, 0o700);

  const temporaryPath = `${filePath}.${process.pid}.${randomUUID()}.tmp`;
  try {
    const handle = await open(temporaryPath, "wx", 0o600);
    try {
      await handle.writeFile(`${JSON.stringify(session)}\n`, "utf8");
      await handle.sync();
      await handle.chmod(0o600);
    } finally {
      await handle.close();
    }
    // rename is the atomic commit point. The destination is never opened for
    // writing, so a partial JSON document cannot become visible to readers.
    await rename(temporaryPath, filePath);
  } finally {
    await rm(temporaryPath, { force: true }).catch(() => {});
  }
}

export async function clearLocalGuestSession(filePath: string): Promise<void> {
  await rm(filePath, { force: true });
}
