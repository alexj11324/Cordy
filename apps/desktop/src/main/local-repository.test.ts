// @vitest-environment node
import { execFileSync } from "node:child_process";
import { mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, describe, expect, it } from "vitest";
import { inspectRepository, repositoryIdentity, classifyGitFailure, cloneRepository, cleanRemote } from "./local-repository";
const roots: string[] = [];
async function fixture() {
  const root = await mkdtemp(join(tmpdir(), "cordy-repo-")); roots.push(root);
  execFileSync("git", ["init", "-q", root]);
  await writeFile(join(root, "code.txt"), "committed\n");
  execFileSync("git", ["-C", root, "add", "."]);
  execFileSync("git", ["-C", root, "-c", "user.name=Test", "-c", "user.email=test@example.com", "commit", "-qm", "initial"]);
  return root;
}
afterEach(async () => { await Promise.all(roots.splice(0).map(root => rm(root, {recursive: true, force: true}))); });
describe("local repository binding", () => {
  it("identifies SSH and HTTPS as the same repository without leaking credentials", () => {
    expect(repositoryIdentity("git@github.com:Team/App.git")).toBe("github.com/team/app");
    expect(repositoryIdentity("https://user:password@github.com/Team/App.git")).toBe("github.com/team/app");
    expect(cleanRemote("ssh://alice@git.example.com/team/app.git")).toBe("ssh://alice@git.example.com/team/app.git");
    expect(repositoryIdentity("/local/folder")).toBeNull();
    expect(repositoryIdentity("alice@git.example.com:team/app.git")).toBe("git.example.com/team/app");
  });
  it("reads all remotes while preserving uncommitted work", async () => {
    const root = await fixture();
    execFileSync("git", ["-C", root, "remote", "add", "origin", "https://user:password@github.com/team/app.git"]);
    execFileSync("git", ["-C", root, "remote", "add", "upstream", "git@github.com:other/app.git"]);
    await writeFile(join(root, "code.txt"), "unfinished\n");
    const result = await inspectRepository(root);
    expect(result.remotes).toEqual([{name:"origin",url:"https://github.com/team/app.git"},{name:"upstream",url:"ssh://git@github.com/other/app.git"}]);
    expect(result.has_commits).toBe(true);
    expect(JSON.stringify(result)).not.toContain("password");
    expect(await readFile(join(root,"code.txt"),"utf8")).toBe("unfinished\n");
  });
  it("never replaces an existing clone destination", async () => {
    const root = await fixture();
    const result = await cloneRepository("https://github.com/team/app.git", root);
    expect(result).toEqual({ok:false, reason:"destination_exists"});
    expect(await readFile(join(root,"code.txt"),"utf8")).toBe("committed\n");
  });
  it("rejects non-network Git transports before running clone", async () => {
    expect(await cloneRepository("ext::sh -c malicious", "/tmp/unused")).toEqual({ok:false,reason:"invalid_url"});
  });
  it("distinguishes authentication, network and ambiguous repository access errors", () => {
    expect(classifyGitFailure("fatal: could not read Username: terminal prompts disabled")).toBe("authentication_required");
    expect(classifyGitFailure("Permission denied (publickey).")).toBe("authentication_required");
    expect(classifyGitFailure("Could not resolve host: github.com")).toBe("network_error");
    expect(classifyGitFailure("Repository not found.")).toBe("repository_unavailable");
    expect(classifyGitFailure("The requested URL returned error: 403")).toBe("access_denied");
  });
});
