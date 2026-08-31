import { execFileSync } from "node:child_process";
import { chmod, mkdtemp, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";

import { afterEach, describe, expect, it } from "vitest";

const repoRoot = resolve(import.meta.dirname, "..", "..", "..");
let sandbox;

afterEach(async () => {
  if (sandbox) await rm(sandbox, { recursive: true, force: true });
  sandbox = undefined;
});

async function fakeExecutable(name, contents) {
  const path = join(sandbox, name);
  await writeFile(path, `#!/bin/bash\n${contents}\n`);
  await chmod(path, 0o755);
  return path;
}

describe("run-rust shared compiler cache", () => {
  it("uses sccache without sharing the worktree target directory", async () => {
    sandbox = await mkdtemp(join(tmpdir(), "patchbay-rust-wrapper-"));
    const cargo = await fakeExecutable(
      "cargo-fixture",
      'printf "%s" "${RUSTC_WRAPPER:-missing}"',
    );
    const sccache = await fakeExecutable("sccache", 'printf "sccache fixture"');

    const output = execFileSync(
      join(repoRoot, "scripts", "run-rust.sh"),
      ["--version"],
      {
        cwd: repoRoot,
        encoding: "utf8",
        env: {
          ...process.env,
          PATH: `${sandbox}:${process.env.PATH}`,
          CARGO_BIN: cargo,
          RUSTC_WRAPPER: "",
        },
      },
    );

    expect(output).toBe(sccache);
  });

  it("preserves an explicit compiler wrapper", async () => {
    sandbox = await mkdtemp(join(tmpdir(), "patchbay-rust-wrapper-"));
    const cargo = await fakeExecutable(
      "cargo-fixture",
      'printf "%s" "${RUSTC_WRAPPER:-missing}"',
    );
    await fakeExecutable("sccache", 'printf "sccache fixture"');

    const output = execFileSync(
      join(repoRoot, "scripts", "run-rust.sh"),
      ["--version"],
      {
        cwd: repoRoot,
        encoding: "utf8",
        env: {
          ...process.env,
          PATH: `${sandbox}:${process.env.PATH}`,
          CARGO_BIN: cargo,
          RUSTC_WRAPPER: "/custom/wrapper",
        },
      },
    );

    expect(output).toBe("/custom/wrapper");
  });
});
