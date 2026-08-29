/**
 * Parse a git config (or `.git` gitdir pointer) without spawning git.
 * Onboarding uses this after the folder picker so a local directory can
 * also attach as `github_repo` when origin is set.
 */

import { readFile, stat } from "fs/promises";
import { dirname, isAbsolute, join, resolve } from "path";

export function parseGitdirPointer(contents: string): string | null {
  const match = contents.match(/^gitdir:\s*(.+)\s*$/m);
  return match?.[1]?.trim() ? match[1].trim() : null;
}

export function parseGitConfigRemoteUrl(
  configText: string,
  remote = "origin",
): string | null {
  const escaped = remote.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
  const header = new RegExp(`^\\[remote\\s+"${escaped}"\\]$`, "i");
  let inSection = false;
  for (const rawLine of configText.split(/\r?\n/)) {
    const line = rawLine.trim();
    if (!line || line.startsWith("#") || line.startsWith(";")) continue;
    if (line.startsWith("[")) {
      inSection = header.test(line);
      continue;
    }
    if (!inSection) continue;
    const match = line.match(/^url\s*=\s*(.+)$/i);
    if (match?.[1]) return match[1].trim();
  }
  return null;
}

/** Walk up from `start` looking for `.git` and read `origin.url`. */
export async function readOriginUrlFromDirectory(
  start: string,
): Promise<string | null> {
  let current = start;
  for (;;) {
    const gitPath = join(current, ".git");
    try {
      const st = await stat(gitPath);
      if (st.isFile()) {
        const pointer = parseGitdirPointer(await readFile(gitPath, "utf8"));
        if (!pointer) return null;
        const gitDir = isAbsolute(pointer) ? pointer : resolve(current, pointer);
        const config = await readFile(join(gitDir, "config"), "utf8");
        return parseGitConfigRemoteUrl(config);
      }
      if (st.isDirectory()) {
        const config = await readFile(join(gitPath, "config"), "utf8");
        return parseGitConfigRemoteUrl(config);
      }
    } catch {
      // Not a git dir here — keep walking up.
    }
    const parent = dirname(current);
    if (parent === current) return null;
    current = parent;
  }
}
