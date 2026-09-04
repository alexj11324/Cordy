// @vitest-environment node

import { createHash } from "node:crypto";
import { chmod, mkdtemp, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const ctx = vi.hoisted(() => ({ userDataPath: "" }));

vi.mock("electron", () => ({
  app: {
    getPath: vi.fn(() => ctx.userDataPath),
    getAppPath: vi.fn(() => "/app"),
  },
  BrowserWindow: class BrowserWindow {},
  ipcMain: { handle: vi.fn() },
}));

import {
  bundledCliPath,
  localGuestChildEnvironment,
  parseBundledCliDigest,
  verifyBundledCli,
} from "./local-guest-runtime";

const temporaryDirectories: string[] = [];

async function createTemporaryDirectory(): Promise<string> {
  const directory = await mkdtemp(join(tmpdir(), "patchbay-cli-"));
  temporaryDirectories.push(directory);
  return directory;
}

/**
 * A stand-in for the bundled `patchbay` binary. `verifyBundledCli` executes
 * the candidate and requires a JSON version banner, so the fake has to be a
 * real executable rather than an inert file.
 */
async function writeFakeCli(
  directory: string,
  body = 'echo \'{"version":"0.1.0"}\'',
): Promise<string> {
  const binaryPath = join(directory, "patchbay");
  await writeFile(binaryPath, `#!/bin/sh\n${body}\n`, { mode: 0o755 });
  await chmod(binaryPath, 0o755);
  return binaryPath;
}

async function writeDigestFor(binaryPath: string, contents: string) {
  const digest = createHash("sha256").update(contents).digest("hex");
  await writeFile(`${binaryPath}.sha256`, `${digest}  patchbay\n`);
}

beforeEach(async () => {
  ctx.userDataPath = await createTemporaryDirectory();
});

afterEach(async () => {
  await Promise.all(
    temporaryDirectories
      .splice(0)
      .map((directory) => rm(directory, { recursive: true, force: true })),
  );
});

describe("bundled CLI location", () => {
  it("reads the unpacked copy, which is the one that can be executed", () => {
    expect(bundledCliPath("/Applications/Patchbay.app/app.asar", "darwin")).toBe(
      "/Applications/Patchbay.app/app.asar.unpacked/resources/bin/patchbay",
    );
    expect(bundledCliPath("C:/app.asar", "win32")).toContain("patchbay.exe");
  });
});

describe("parseBundledCliDigest", () => {
  it("accepts a sha256sum-style line and nothing else", () => {
    expect(
      parseBundledCliDigest(`${"a".repeat(64)}  patchbay\n`),
    ).toBe("a".repeat(64));
    expect(parseBundledCliDigest(`${"A".repeat(64)}`)).toBe("a".repeat(64));
    expect(parseBundledCliDigest("")).toBeNull();
    expect(parseBundledCliDigest("not-a-digest  patchbay")).toBeNull();
    // A truncated digest must not be accepted as a prefix match.
    expect(parseBundledCliDigest("a".repeat(63))).toBeNull();
  });
});

describe("verifyBundledCli", () => {
  it("accepts the bundled binary when the recorded digest matches", async () => {
    const directory = await createTemporaryDirectory();
    const body = 'echo \'{"version":"0.1.0"}\'';
    const binaryPath = await writeFakeCli(directory, body);
    await writeDigestFor(binaryPath, `#!/bin/sh\n${body}\n`);

    await expect(verifyBundledCli(binaryPath)).resolves.toBe(true);
  });

  it("refuses a binary whose bytes no longer match the recorded digest", async () => {
    // This is the whole point of shipping the digest: an attacker who can
    // replace the bundled binary owns every local Guest run.
    const directory = await createTemporaryDirectory();
    const original = 'echo \'{"version":"0.1.0"}\'';
    const binaryPath = await writeFakeCli(directory, original);
    await writeDigestFor(binaryPath, `#!/bin/sh\n${original}\n`);

    await writeFakeCli(directory, 'echo \'{"version":"9.9.9"}\'; touch pwned');

    await expect(verifyBundledCli(binaryPath)).resolves.toBe(false);
  });

  it("refuses to run anything when the digest file is missing or malformed", async () => {
    const directory = await createTemporaryDirectory();
    const binaryPath = await writeFakeCli(directory);

    await expect(verifyBundledCli(binaryPath)).resolves.toBe(false);

    await writeFile(`${binaryPath}.sha256`, "garbage\n");
    await expect(verifyBundledCli(binaryPath)).resolves.toBe(false);
  });

  it("refuses a binary that is not executable", async () => {
    const directory = await createTemporaryDirectory();
    const body = 'echo \'{"version":"0.1.0"}\'';
    const binaryPath = await writeFakeCli(directory, body);
    await writeDigestFor(binaryPath, `#!/bin/sh\n${body}\n`);
    await chmod(binaryPath, 0o644);

    await expect(verifyBundledCli(binaryPath)).resolves.toBe(false);
  });

  it("refuses a binary that does not answer the version probe", async () => {
    const directory = await createTemporaryDirectory();
    const body = "exit 3";
    const binaryPath = await writeFakeCli(directory, body);
    await writeDigestFor(binaryPath, `#!/bin/sh\n${body}\n`);

    await expect(verifyBundledCli(binaryPath)).resolves.toBe(false);
  });

  it("refuses a missing binary", async () => {
    const directory = await createTemporaryDirectory();

    await expect(
      verifyBundledCli(join(directory, "absent")),
    ).resolves.toBe(false);
  });
});

describe("localGuestChildEnvironment", () => {
  it("hands the local runner no Patchbay configuration at all", async () => {
    // PATCHBAY_* is how the CLI is pointed at a server, a profile or a token.
    // Inheriting even one of them would turn a local Guest run into a cloud
    // call, which is exactly what Guest mode exists to prevent.
    vi.stubEnv("PATCHBAY_SERVER_URL", "https://api.aspectlylabs.com");
    vi.stubEnv("PATCHBAY_TOKEN", "secret");
    vi.stubEnv("PATCHBAY_PROFILE", "work");
    try {
      const environment = await localGuestChildEnvironment();

      expect(
        Object.keys(environment).filter((key) => key.startsWith("PATCHBAY")),
      ).toEqual([]);
    } finally {
      vi.unstubAllEnvs();
    }
  });

  it("redirects HOME into the Guest runtime directory so no profile is reachable", async () => {
    const environment = await localGuestChildEnvironment();

    expect(environment.HOME).toBe(
      join(ctx.userDataPath, "local-guest", "runtime-home"),
    );
    expect(environment.USERPROFILE).toBe(environment.HOME);
    // Windows composes a home directory from these two; leaving them set
    // would reopen the real profile the HOME override just closed.
    expect(environment.HOMEDRIVE).toBeUndefined();
    expect(environment.HOMEPATH).toBeUndefined();
  });

  it("passes through only command lookup and locale variables", async () => {
    vi.stubEnv("AWS_SECRET_ACCESS_KEY", "leak-me");
    vi.stubEnv("GITHUB_TOKEN", "leak-me");
    try {
      const environment = await localGuestChildEnvironment();

      expect(environment.AWS_SECRET_ACCESS_KEY).toBeUndefined();
      expect(environment.GITHUB_TOKEN).toBeUndefined();
      expect(environment.PATH).toBe(process.env.PATH);
    } finally {
      vi.unstubAllEnvs();
    }
  });
});
