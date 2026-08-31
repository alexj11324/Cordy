import { readFileSync } from "node:fs";
import { join } from "node:path";

import { describe, expect, it } from "vitest";

import { planCompleteDevLauncher } from "../../../scripts/dev-launcher.mjs";

describe("cross-platform complete development entrypoint", () => {
  it("bootstraps secure Clerk auth before spawning dependency or Rust work", () => {
    const source = readFileSync(
      join(import.meta.dirname, "../../../scripts/dev-launcher.mjs"),
      "utf8",
    );
    const auth = source.indexOf("bootstrapDevClerkAuth({ env })");
    const spawn = source.indexOf("child = spawn(");
    expect(auth).toBeGreaterThan(-1);
    expect(spawn).toBeGreaterThan(auth);
  });

  it("uses the POSIX implementation and forwards Electron arguments", () => {
    expect(
      planCompleteDevLauncher("darwin", ["--inspect"], {
        repoRoot: "/repo",
      }),
    ).toEqual({
      command: "bash",
      args: [join("/repo", "scripts", "dev.sh"), "--inspect"],
    });
  });

  it("uses native Windows PowerShell without requiring Bash", () => {
    expect(
      planCompleteDevLauncher("win32", ["--inspect"], {
        repoRoot: "C:\\repo",
      }),
    ).toEqual({
      command: "powershell.exe",
      args: [
        "-NoProfile",
        "-ExecutionPolicy",
        "Bypass",
        "-File",
        join("C:\\repo", "scripts", "dev.ps1"),
        "--inspect",
      ],
    });
  });

  it("forwards hosted mode to the platform launcher", () => {
    expect(
      planCompleteDevLauncher("darwin", ["--hosted"], {
        repoRoot: "/repo",
      }),
    ).toEqual({
      command: "bash",
      args: [join("/repo", "scripts", "dev.sh"), "--hosted"],
    });
  });
});
