#!/usr/bin/env node

import { createHash } from "node:crypto";
import { execFileSync } from "node:child_process";
import {
  access,
  chmod,
  copyFile,
  mkdir,
  mkdtemp,
  readFile,
  readdir,
  rm,
  writeFile,
} from "node:fs/promises";
import { basename, dirname, join, resolve } from "node:path";
import { tmpdir } from "node:os";
import { fileURLToPath, pathToFileURL } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const repoRoot = resolve(here, "..");
const serverRsDir = join(repoRoot, "server-rs");
const cargoTargetDir = process.env.CARGO_TARGET_DIR
  ? resolve(serverRsDir, process.env.CARGO_TARGET_DIR)
  : join(serverRsDir, "target");

const TARGETS = Object.freeze({
  darwin: Object.freeze({
    amd64: "x86_64-apple-darwin",
    arm64: "aarch64-apple-darwin",
  }),
  linux: Object.freeze({
    amd64: "x86_64-unknown-linux-gnu",
    arm64: "aarch64-unknown-linux-gnu",
  }),
  windows: Object.freeze({
    amd64: "x86_64-pc-windows-msvc",
    arm64: "aarch64-pc-windows-msvc",
  }),
});

const RELEASE_TARGETS = Object.freeze(
  Object.entries(TARGETS).flatMap(([platform, arches]) =>
    Object.keys(arches).map((arch) => ({ platform, arch })),
  ),
);

function binaryName(platform) {
  return platform === "windows" ? "cordy.exe" : "cordy";
}

function archiveExtension(platform) {
  return platform === "windows" ? "zip" : "tar.gz";
}

export function rustTargetFor(platform, arch) {
  const target = TARGETS[platform]?.[arch];
  if (!target) {
    throw new Error(
      `unsupported release target ${platform}/${arch}; expected darwin, linux, or windows with amd64 or arm64`,
    );
  }
  return target;
}

function git(...args) {
  try {
    return execFileSync("git", args, { cwd: repoRoot, encoding: "utf8" }).trim();
  } catch {
    return "";
  }
}

function buildVersion(options) {
  return (
    options.version ||
    process.env.GITHUB_REF_NAME?.replace(/^v/, "") ||
    git("describe", "--tags", "--match", "v[0-9]*", "--always") ||
    "dev"
  );
}

export function releaseVersion(options) {
  const version = buildVersion(options);
  if (!/^\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?$/.test(version)) {
    throw new Error(`release version is not valid semver: ${version}`);
  }
  return version;
}

function releaseTag(options, version) {
  return options.tag ?? process.env.GITHUB_REF_NAME ?? `v${version}`;
}

function defaultOutputDir(name) {
  return join(process.env.RUNNER_TEMP || tmpdir(), name);
}

function parseOptionValue(argv, index, option) {
  const value = argv[index + 1];
  if (!value || value.startsWith("--")) {
    throw new Error(`${option} requires a value`);
  }
  return value;
}

export function parseArgs(argv) {
  const command = argv[0] ?? "help";
  const options = {};
  for (let index = 1; index < argv.length; index += 1) {
    const token = argv[index];
    const equals = token.indexOf("=");
    const option = equals === -1 ? token : token.slice(0, equals);
    const inlineValue = equals === -1 ? undefined : token.slice(equals + 1);
    if (!token.startsWith("--")) {
      throw new Error(`unexpected argument: ${token}`);
    }
    const key = option.slice(2).replaceAll("-", "_");
    const value = inlineValue ?? parseOptionValue(argv, index++, option);
    options[key] = value;
  }
  return { command, options };
}

function buildEnvironment(version) {
  const commit =
    process.env.GITHUB_SHA?.slice(0, 7) || git("rev-parse", "--short", "HEAD") || "unknown";
  return {
    ...process.env,
    CARGO_TARGET_DIR: cargoTargetDir,
    CORDY_BUILD_VERSION: version,
    CORDY_BUILD_COMMIT: commit,
    CORDY_BUILD_DATE: new Date().toISOString().replace(/\.\d+Z$/, "Z"),
    CORDY_BUILD_GO_VERSION: "unknown",
    CORDY_GIT_COMMIT: commit,
  };
}

async function ensureFile(path, description) {
  try {
    await access(path);
  } catch {
    throw new Error(`${description} not found: ${path}`);
  }
}

export async function buildTarget({ platform, arch, outputDir, version }) {
  const target = rustTargetFor(platform, arch);
  const executable = binaryName(platform);
  const targetBinary = join(cargoTargetDir, target, "release", executable);
  await mkdir(outputDir, { recursive: true });
  console.log(
    `[release-cli] cargo build --release --locked -p cordy-cli --target ${target}`,
  );
  execFileSync(
    "cargo",
    ["build", "--release", "--locked", "-p", "cordy-cli", "--target", target],
    { cwd: serverRsDir, env: buildEnvironment(version), stdio: "inherit" },
  );
  await ensureFile(targetBinary, "Rust CLI release binary");
  const output = join(outputDir, `cordy-${platform}-${arch}${platform === "windows" ? ".exe" : ""}`);
  await copyFile(targetBinary, output);
  console.log(`[release-cli] staged ${output}`);
  return output;
}

async function findFile(root, wanted) {
  const entries = await readdir(root, { withFileTypes: true });
  for (const entry of entries.sort((left, right) => left.name.localeCompare(right.name))) {
    const path = join(root, entry.name);
    if (entry.isFile() && entry.name === wanted) return path;
    if (entry.isDirectory()) {
      const found = await findFile(path, wanted);
      if (found) return found;
    }
  }
  return null;
}

async function copyReleaseDocs(stage) {
  const entries = await readdir(repoRoot, { withFileTypes: true });
  const docs = entries
    .filter(
      (entry) =>
        entry.isFile() && /^(LICENSE|README|NOTICE)/.test(entry.name),
    )
    .map((entry) => entry.name)
    .sort();
  for (const doc of docs) {
    await copyFile(join(repoRoot, doc), join(stage, doc));
  }
  return docs;
}

async function archiveStage(stage, files, archivePath, platform) {
  if (platform === "windows") {
    execFileSync("zip", ["-q", archivePath, ...files], {
      cwd: stage,
      stdio: "inherit",
    });
    return;
  }
  execFileSync("tar", ["-czf", archivePath, "-C", stage, ...files], {
    stdio: "inherit",
  });
}

function archiveName(version, platform, arch, legacy) {
  const stem = legacy
    ? `cordy_${platform}_${arch}`
    : `cordy-cli-${version}-${platform}-${arch}`;
  return `${stem}.${archiveExtension(platform)}`;
}

async function sha256(path) {
  return createHash("sha256").update(await readFile(path)).digest("hex");
}

export async function packageRelease({ inputDir, outputDir, version }) {
  await rm(outputDir, { recursive: true, force: true });
  await mkdir(outputDir, { recursive: true });
  const archives = [];

  for (const { platform, arch } of RELEASE_TARGETS) {
    const rawName = `cordy-${platform}-${arch}${platform === "windows" ? ".exe" : ""}`;
    const source = await findFile(inputDir, rawName);
    if (!source) {
      throw new Error(`Rust CLI build artifact is missing: ${rawName}`);
    }
    const stage = await mkdtemp(join(tmpdir(), "cordy-cli-stage-"));
    try {
      const executable = binaryName(platform);
      await copyFile(source, join(stage, executable));
      // actions/upload-artifact does not promise to preserve POSIX mode bits;
      // restore the executable bit before tar records the Linux/macOS mode.
      await chmod(join(stage, executable), 0o755);
      const docs = await copyReleaseDocs(stage);
      const files = [executable, ...docs];
      for (const legacy of [true, false]) {
        const name = archiveName(version, platform, arch, legacy);
        const path = join(outputDir, name);
        await rm(path, { force: true });
        await archiveStage(stage, files, path, platform);
        archives.push(path);
      }
    } finally {
      await rm(stage, { recursive: true, force: true });
    }
  }

  const checksums = await Promise.all(
    archives.map(async (path) => `${await sha256(path)}  ${basename(path)}`),
  );
  await writeFile(join(outputDir, "checksums.txt"), `${checksums.sort().join("\n")}\n`);
  console.log(`[release-cli] packaged ${archives.length} archives in ${outputDir}`);
  return archives;
}

function parseChecksumManifest(text) {
  return new Map(
    text
      .split(/\r?\n/)
      .filter(Boolean)
      .map((line) => {
        const match = /^(?<hash>[0-9a-f]{64})  (?<name>.+)$/.exec(line);
        if (!match) throw new Error(`invalid checksum line: ${line}`);
        return [match.groups.name, match.groups.hash];
      }),
  );
}

function gh(args, token) {
  return execFileSync("gh", args, {
    encoding: "utf8",
    env: { ...process.env, GH_TOKEN: token },
    stdio: ["ignore", "pipe", "inherit"],
  }).trim();
}

function homebrewFormula(version, tag, checksums) {
  const archive = (platform, arch) => archiveName(version, platform, arch, false);
  const sha = (platform, arch) => {
    const name = archive(platform, arch);
    const value = checksums.get(name);
    if (!value) throw new Error(`checksum missing for Homebrew archive: ${name}`);
    return value;
  };
  const url = (platform, arch) =>
    `https://github.com/cordy-ai/cordy/releases/download/${tag}/${archive(platform, arch)}`;

  return `class Cordy < Formula
  desc "Cordy CLI — local agent runtime and management tool for the Cordy platform"
  homepage "https://github.com/cordy-ai/cordy"
  version "${version}"

  on_macos do
    if Hardware::CPU.arm?
      url "${url("darwin", "arm64")}"
      sha256 "${sha("darwin", "arm64")}"
    else
      url "${url("darwin", "amd64")}"
      sha256 "${sha("darwin", "amd64")}"
    end
  end

  on_linux do
    if Hardware::CPU.arm?
      url "${url("linux", "arm64")}"
      sha256 "${sha("linux", "arm64")}"
    else
      url "${url("linux", "amd64")}"
      sha256 "${sha("linux", "amd64")}"
    end
  end

  def install
    bin.install "cordy"
  end

  test do
    system "#{bin}/cordy", "version"
  end
end
`;
}

export async function publishHomebrew({ outputDir, version, tag }) {
  const token = process.env.HOMEBREW_TAP_GITHUB_TOKEN;
  if (!token) throw new Error("HOMEBREW_TAP_GITHUB_TOKEN is required to publish the formula");
  const manifest = parseChecksumManifest(
    await readFile(join(outputDir, "checksums.txt"), "utf8"),
  );
  const formula = homebrewFormula(version, tag, manifest);
  const endpoint = "repos/cordy-ai/homebrew-tap/contents/Formula/cordy.rb";
  let current;
  try {
    current = JSON.parse(gh(["api", `${endpoint}?ref=main`], token));
  } catch {
    current = null;
  }
  const args = [
    "api",
    "--method",
    "PUT",
    endpoint,
    "--field",
    `message=Update cordy to ${version}`,
    "--field",
    `content=${Buffer.from(formula).toString("base64")}`,
    "--field",
    "branch=main",
  ];
  if (current?.sha) args.push("--field", `sha=${current.sha}`);
  gh(args, token);
  console.log("[release-cli] updated cordy-ai/homebrew-tap Formula/cordy.rb");
}

function usage() {
  console.log(`Usage:
  release-cli.mjs build --platform <darwin|linux|windows> --arch <amd64|arm64>
  release-cli.mjs package [--input-dir <dir>] [--output-dir <dir>] [--version <semver>]
  release-cli.mjs publish-homebrew [--output-dir <dir>] [--version <semver>] [--tag <tag>]
`);
}

async function main() {
  const { command, options } = parseArgs(process.argv.slice(2));
  if (command === "help" || command === "--help") {
    usage();
    return;
  }
  if (command === "build") {
    const platform = options.platform;
    const arch = options.arch;
    if (!platform || !arch) throw new Error("build requires --platform and --arch");
    await buildTarget({
      platform,
      arch,
      outputDir: options.output_dir || defaultOutputDir("cordy-cli"),
      version: buildVersion(options),
    });
    return;
  }
  if (command === "package") {
    const version = releaseVersion(options);
    await packageRelease({
      inputDir: options.input_dir || defaultOutputDir("cordy-cli"),
      outputDir: options.output_dir || defaultOutputDir("cordy-release"),
      version,
    });
    return;
  }
  if (command === "publish-homebrew") {
    const version = releaseVersion(options);
    await publishHomebrew({
      outputDir: options.output_dir || defaultOutputDir("cordy-release"),
      version,
      tag: releaseTag(options, version),
    });
    return;
  }
  throw new Error(`unknown command: ${command}`);
}

if (
  process.argv[1] &&
  import.meta.url === pathToFileURL(process.argv[1]).href
) {
  main().catch((error) => {
    console.error(`[release-cli] ${error.message}`);
    process.exitCode = 1;
  });
}
