import {
  chmod,
  lstat,
  readdir,
} from "node:fs/promises";
import { join } from "node:path";

const PRIVATE_PROFILE_FILES = [
  "config.json",
  ".desktop-user-id",
  ".config.lock",
];

/** Repair Desktop profiles created before private modes were enforced. */
export async function hardenExistingDesktopProfiles(
  profilesRoot: string,
): Promise<number> {
  if (process.platform === "win32") return 0;
  let entries;
  try {
    entries = await readdir(profilesRoot, { withFileTypes: true });
  } catch (error) {
    if ((error as NodeJS.ErrnoException).code === "ENOENT") return 0;
    throw error;
  }

  let hardened = 0;
  for (const entry of entries) {
    if (!entry.isDirectory() || !/^desktop(?:-[a-z0-9.-]+)?$/.test(entry.name)) {
      continue;
    }
    const directory = join(profilesRoot, entry.name);
    await chmod(directory, 0o700);
    for (const name of PRIVATE_PROFILE_FILES) {
      const filePath = join(directory, name);
      try {
        const file = await lstat(filePath);
        if (file.isFile()) await chmod(filePath, 0o600);
      } catch (error) {
        if ((error as NodeJS.ErrnoException).code !== "ENOENT") throw error;
      }
    }
    hardened += 1;
  }
  return hardened;
}
