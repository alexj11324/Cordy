import { lstat, mkdir } from "node:fs/promises";
import { isAbsolute } from "node:path";

export type RepositoryFailure = "invalid_url" | "destination_exists" | "authentication_required" | "access_denied" | "repository_unavailable" | "network_error" | "error";
export type RepositoryResult = { ok: true } | { ok: false; reason: RepositoryFailure };

/** Never persist credentials from a user's configured remote in shared resources. */
export function cleanRemote(value: string): string | null {
  const scp = /^([^@/:\s]+)@([^/:\s]+):([^\s]+)$/.exec(value);
  try {
    const url = new URL(scp ? `ssh://${scp[1]}@${scp[2]}/${scp[3]}` : value);
    if (!["https:", "ssh:"].includes(url.protocol) || !url.hostname || url.pathname === "/") return null;
    if (url.protocol !== "ssh:") url.username = "";
    url.password = "";
    url.search = "";
    url.hash = "";
    return url.toString();
  } catch { return null; }
}
export function repositoryIdentity(value: string): string | null {
  const clean = cleanRemote(value);
  if (!clean) return null;
  const url = new URL(clean);
  const identity = `${url.host}${url.pathname.replace(/\/$/, "").replace(/\.git$/, "")}`;
  return url.hostname === "github.com" ? identity.toLowerCase() : identity;
}

async function git(args: string[], timeout = 30_000): Promise<{ stdout: string; stderr: string }> {
  // Resolve child_process only when a repository operation is requested. Some
  // desktop tests mock the module with only the process primitive they exercise;
  // importing execFile eagerly would make unrelated local Guest tests fail at
  // module load time.
  const { execFile } = await import("node:child_process");
  if (typeof execFile !== "function") {
    throw new Error("Git process execution is unavailable in this desktop runtime");
  }
  return new Promise((resolve, reject) => {
    execFile(
      "git",
      args,
      {
        timeout,
        maxBuffer: 1024 * 1024,
        env: {
          ...process.env,
          GIT_TERMINAL_PROMPT: "0",
          GCM_INTERACTIVE: "never",
          GIT_SSH_COMMAND: "ssh -o BatchMode=yes -o StrictHostKeyChecking=yes",
        },
      },
      (error, stdout, stderr) => {
        if (error) {
          reject(Object.assign(error, { stdout, stderr }));
          return;
        }
        resolve({ stdout, stderr });
      },
    );
  });
}
export async function inspectRepository(path: string) {
  const remotes: Array<{name: string; url: string}> = [];
  if (!isAbsolute(path)) return {remotes, has_commits: false};
  try {
    const {stdout} = await git(["-C", path, "remote"]);
    for (const name of stdout.trim().split("\n").filter(Boolean)) {
      const result = await git(["-C", path, "remote", "get-url", name]);
      const url = cleanRemote(result.stdout.trim());
      if (url) remotes.push({name, url});
    }
    await git(["-C", path, "rev-parse", "--verify", "HEAD"]);
    return {remotes, has_commits: true};
  } catch { return {remotes, has_commits: false}; }
}
export function classifyGitFailure(message: string): RepositoryFailure {
  if (/could not resolve|failed to connect|connection.*(?:timed out|refused|reset)|network is unreachable|unable to access.*SSL/i.test(message)) return "network_error";
  if (/authentication failed|could not read (?:Username|Password)|terminal prompts disabled|permission denied \(publickey\)/i.test(message)) return "authentication_required";
  if (/403|access denied|write access.*not granted/i.test(message)) return "access_denied";
  if (/repository.*not found|does not appear to be a git repository/i.test(message)) return "repository_unavailable";
  return "error";
}
export async function checkRepositoryAccess(value: string): Promise<RepositoryResult> {
  const url = cleanRemote(value);
  if (!url || url !== value.trim() && /:\/\/[^/]*:[^/@]*@/.test(value)) return {ok:false,reason:"invalid_url"};
  try { await git(["ls-remote", "--", url]); return {ok:true}; }
  catch (error) { return {ok:false,reason:classifyGitFailure(String(error))}; }
}
export async function cloneRepository(value: string, destination: string): Promise<RepositoryResult> {
  const url = cleanRemote(value);
  if (!url || !isAbsolute(destination)) return {ok:false,reason:"invalid_url"};
  try { await lstat(destination); return {ok:false,reason:"destination_exists"}; }
  catch (error) { if ((error as NodeJS.ErrnoException).code !== "ENOENT") return {ok:false,reason:"error"}; }
  // mkdir is exclusive: a concurrent creator must never have its files adopted.
  try { await mkdir(destination); }
  catch (error) { return {ok:false,reason:(error as NodeJS.ErrnoException).code === "EEXIST" ? "destination_exists" : "error"}; }
  try { await git(["clone", "--", url, destination], 300_000); return {ok:true}; }
  catch (error) {
    // Keep a partial destination for inspection. Never recursively delete a
    // folder the user could have started working in while clone was running.
    return {ok:false,reason:classifyGitFailure(String(error))};
  }
}
