import { access, mkdir, mkdtemp, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { afterEach, describe, expect, it } from "vitest";

import { clearDevCliArtifact } from "./clear-dev-cli.mjs";

let sandbox;

afterEach(async () => {
  if (sandbox) await rm(sandbox, { recursive: true, force: true });
  sandbox = undefined;
});

describe("clearDevCliArtifact", () => {
  it("removes a source CLI left by an earlier Rust development run", async () => {
    sandbox = await mkdtemp(join(tmpdir(), "patchbay-dev-cli-"));
    const artifactDir = join(sandbox, "resources", "bin");
    await mkdir(artifactDir, { recursive: true });
    await writeFile(join(artifactDir, "patchbay"), "stale source binary");

    await clearDevCliArtifact(artifactDir);

    await expect(access(artifactDir)).rejects.toMatchObject({ code: "ENOENT" });
  });
});
