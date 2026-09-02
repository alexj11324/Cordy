import { realpath } from "node:fs/promises";
import { isAbsolute, relative, resolve } from "node:path";

/**
 * True when `candidate` is `root` itself or lives inside it.
 *
 * Both arguments must already be absolute. `relative()` is the containment
 * test rather than a string prefix compare, because `/home/user/work` is a
 * string prefix of `/home/user/workspace-secrets` but is not its parent.
 */
export function isPathWithin(root: string, candidate: string): boolean {
  const normalizedRoot = resolve(root);
  const normalizedCandidate = resolve(candidate);
  if (normalizedRoot === normalizedCandidate) return true;
  const rest = relative(normalizedRoot, normalizedCandidate);
  return rest !== "" && !rest.startsWith("..") && !isAbsolute(rest);
}

/**
 * The set of directories the user actually chose, owned by the main process.
 *
 * The renderer sends a working directory string with every Guest run, and a
 * renderer string is not consent: on its own it would let any compromised or
 * buggy renderer point the local runner at `$HOME/.ssh` or at a cloud
 * profile directory. A path is only runnable here after it was handed back by
 * the OS directory picker — the one moment the user expressed intent — or
 * because it sits inside such a directory.
 *
 * Both sides of the comparison are resolved through `realpath`, so neither a
 * `..` segment nor a symlink planted inside a granted directory can be used
 * to escape the grant.
 */
export class LocalWorkspaceGrants {
  readonly #roots = new Set<string>();

  /**
   * Records a directory the user selected. Returns the resolved path, or null
   * when it is not an absolute, existing path.
   */
  async grant(path: string): Promise<string | null> {
    if (!path || !isAbsolute(path)) return null;
    let resolved: string;
    try {
      resolved = await realpath(path);
    } catch {
      return null;
    }
    this.#roots.add(resolved);
    return resolved;
  }

  /**
   * Resolves a renderer-supplied path to the real path a run may use, or null
   * when no grant covers it. Callers must spawn against the returned path, not
   * the requested one, so the child cannot follow a symlink the check already
   * resolved away.
   */
  async resolveGranted(path: string): Promise<string | null> {
    if (!path || !isAbsolute(path)) return null;
    let resolved: string;
    try {
      resolved = await realpath(path);
    } catch {
      return null;
    }
    for (const root of this.#roots) {
      if (isPathWithin(root, resolved)) return resolved;
    }
    return null;
  }

  /** Drops every grant. Used when a Guest session ends. */
  clear(): void {
    this.#roots.clear();
  }

  get size(): number {
    return this.#roots.size;
  }
}
