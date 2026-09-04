// @vitest-environment node

import { mkdir, mkdtemp, rm, symlink, writeFile } from "node:fs/promises";
import { realpath } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, describe, expect, it } from "vitest";
import { LocalWorkspaceGrants, isPathWithin } from "./local-guest-workspace";

const temporaryDirectories: string[] = [];

afterEach(async () => {
  await Promise.all(
    temporaryDirectories
      .splice(0)
      .map((directory) => rm(directory, { recursive: true, force: true })),
  );
});

async function createTemporaryRoot(): Promise<string> {
  const directory = await mkdtemp(join(tmpdir(), "patchbay-grant-"));
  temporaryDirectories.push(directory);
  // macOS resolves /tmp through a symlink; compare against real paths only.
  return realpath(directory);
}

describe("isPathWithin", () => {
  it("accepts the root itself and anything under it", () => {
    expect(isPathWithin("/home/u/work", "/home/u/work")).toBe(true);
    expect(isPathWithin("/home/u/work", "/home/u/work/src/main.ts")).toBe(true);
  });

  it("rejects a sibling that merely shares a string prefix", () => {
    // The bug a naive startsWith() check ships with.
    expect(isPathWithin("/home/u/work", "/home/u/work-secrets")).toBe(false);
  });

  it("rejects parent and unrelated paths", () => {
    expect(isPathWithin("/home/u/work", "/home/u")).toBe(false);
    expect(isPathWithin("/home/u/work", "/etc/shadow")).toBe(false);
    expect(isPathWithin("/home/u/work", "/home/u/work/../../.ssh")).toBe(false);
  });
});

describe("LocalWorkspaceGrants", () => {
  it("refuses any directory the user never chose", async () => {
    const grants = new LocalWorkspaceGrants();
    const chosen = await createTemporaryRoot();
    const elsewhere = await createTemporaryRoot();

    await grants.grant(chosen);

    expect(await grants.resolveGranted(chosen)).toBe(chosen);
    // The core isolation property: a renderer-supplied path that is a
    // perfectly valid, readable, writable directory is still rejected.
    expect(await grants.resolveGranted(elsewhere)).toBeNull();
    expect(await grants.resolveGranted("/etc")).toBeNull();
  });

  it("accepts a subdirectory of a chosen directory", async () => {
    const grants = new LocalWorkspaceGrants();
    const root = await createTemporaryRoot();
    const nested = join(root, "packages", "core");
    await mkdir(nested, { recursive: true });

    await grants.grant(root);

    expect(await grants.resolveGranted(nested)).toBe(nested);
  });

  it("rejects a traversal that climbs out of the grant", async () => {
    const grants = new LocalWorkspaceGrants();
    const root = await createTemporaryRoot();
    const outside = await createTemporaryRoot();
    await mkdir(join(root, "inner"), { recursive: true });

    await grants.grant(root);

    expect(
      await grants.resolveGranted(join(root, "inner", "..", "..")),
    ).toBeNull();
    expect(await grants.resolveGranted(`${root}/../`)).toBeNull();
    expect(await grants.resolveGranted(outside)).toBeNull();
  });

  it("rejects a symlink planted inside the grant that points outside it", async () => {
    const grants = new LocalWorkspaceGrants();
    const root = await createTemporaryRoot();
    const secrets = await createTemporaryRoot();
    await writeFile(join(secrets, "id_rsa"), "private", { mode: 0o600 });
    const escape = join(root, "escape");
    await symlink(secrets, escape, "dir");

    await grants.grant(root);

    // Resolving before comparing is what makes this fail closed. A check on
    // the literal string would have accepted `<root>/escape`.
    expect(await grants.resolveGranted(escape)).toBeNull();
  });

  it("returns the resolved real path, so callers never spawn on the raw input", async () => {
    const grants = new LocalWorkspaceGrants();
    const root = await createTemporaryRoot();
    const nested = join(root, "project");
    await mkdir(nested);
    const link = join(root, "project-link");
    await symlink(nested, link, "dir");

    await grants.grant(root);

    expect(await grants.resolveGranted(link)).toBe(nested);
  });

  it("rejects relative paths and paths that do not exist", async () => {
    const grants = new LocalWorkspaceGrants();
    const root = await createTemporaryRoot();
    await grants.grant(root);

    expect(await grants.resolveGranted("relative/path")).toBeNull();
    expect(await grants.resolveGranted("")).toBeNull();
    expect(await grants.resolveGranted(join(root, "missing"))).toBeNull();
  });

  it("does not grant a relative or missing directory", async () => {
    const grants = new LocalWorkspaceGrants();

    expect(await grants.grant("relative")).toBeNull();
    expect(await grants.grant("/definitely/not/here")).toBeNull();
    expect(grants.size).toBe(0);
  });

  it("forgets every grant when the session ends", async () => {
    const grants = new LocalWorkspaceGrants();
    const root = await createTemporaryRoot();
    await grants.grant(root);

    grants.clear();

    expect(grants.size).toBe(0);
    expect(await grants.resolveGranted(root)).toBeNull();
  });
});
