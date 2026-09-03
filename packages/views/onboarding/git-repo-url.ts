export type ParsedGitRepo = {
  url: string;
  name: string;
};

function repoNameFromPath(path: string): string | null {
  const name = path
    .replace(/\.git$/i, "")
    .split("/")
    .filter(Boolean)
    .pop();
  return name && name.length > 0 ? name : null;
}

/**
 * Accept the same http(s)/ssh/git URLs the server will store as
 * `github_repo`. Used by the onboarding workspace step so a pasted
 * string can become a project before we hit createProject.
 */
export function parseGitRepoUrl(raw: string): ParsedGitRepo | null {
  const trimmed = raw.trim();
  if (!trimmed || /\s/.test(trimmed)) return null;

  const scp = trimmed.match(/^git@([^:]+):(.+)$/i);
  if (scp?.[1] && scp[2]) {
    const name = repoNameFromPath(scp[2]);
    if (!name) return null;
    return { url: trimmed, name };
  }

  try {
    const url = new URL(trimmed);
    if (!["http:", "https:", "ssh:", "git:"].includes(url.protocol)) {
      return null;
    }
    if (!url.hostname) return null;
    const name = repoNameFromPath(url.pathname);
    if (!name) return null;
    return { url: trimmed, name };
  } catch {
    return null;
  }
}
