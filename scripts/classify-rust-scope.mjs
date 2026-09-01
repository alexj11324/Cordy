import { readFile } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";

const BROAD_FILES = new Set([
  "Cargo.toml",
  "Cargo.lock",
  "rust-toolchain.toml",
  "Makefile",
  "Dockerfile",
  "docker/entrypoint.sh",
  "scripts/run-rust.sh",
  "scripts/check.sh",
  "scripts/makefile-build.test.sh",
  "scripts/auth-broker-helm.test.sh",
  "scripts/helm-config.test.sh",
  "scripts/macos-release-contract.test.sh",
  "scripts/verify-release-tag.sh",
  "scripts/verify-release-tag.test.sh",
  ".github/workflows/release.yml",
  ".github/workflows/macos-release.yml",
]);

const BROAD_PREFIXES = [
  "migrations/",
  "deploy/",
  "docker/",
  "scripts/",
  "server-rs/.sqlx/",
  "server-rs/route-contract/",
];

function normalizeFile(file) {
  return file.trim().replaceAll("\\", "/").replace(/^\.\//u, "");
}

function displayFile(file) {
  return JSON.stringify(file);
}

function relativeManifestPath(manifestPath, repoRoot) {
  const normalized = manifestPath.replaceAll("\\", "/");
  if (!path.isAbsolute(manifestPath)) {
    return normalized.replace(/^\.\//u, "");
  }
  return path
    .relative(repoRoot, manifestPath)
    .replaceAll(path.sep, "/")
    .replace(/^\.\//u, "");
}

function isRustRelevant(file) {
  return (
    file.startsWith("server-rs/") ||
    file.startsWith("migrations/") ||
    file.startsWith("deploy/") ||
    file.startsWith("scripts/") ||
    file.startsWith("docker/") ||
    BROAD_FILES.has(file)
  );
}

function isBroadChange(file) {
  return (
    BROAD_FILES.has(file) ||
    BROAD_PREFIXES.some((prefix) => file.startsWith(prefix)) ||
    file === "server-rs/Cargo.toml" ||
    file === "server-rs/Cargo.lock" ||
    (file.startsWith("server-rs/") && !file.startsWith("server-rs/crates/")) ||
    (file.startsWith("server-rs/crates/") && path.posix.basename(file) === "Cargo.toml") ||
    (file.startsWith("server-rs/crates/") && path.posix.basename(file) === "Cargo.lock")
  );
}

function workspacePackages(metadata) {
  const workspaceMembers = new Set(metadata.workspace_members ?? []);
  return (metadata.packages ?? []).filter((pkg) => workspaceMembers.has(pkg.id));
}

function packageForFile(file, packages, repoRoot) {
  const matches = packages.filter((pkg) => {
    const manifest = relativeManifestPath(pkg.manifest_path, repoRoot);
    const root = manifest.slice(0, -"Cargo.toml".length);
    return file === manifest || file.startsWith(root);
  });
  return matches.length === 1 ? matches[0] : undefined;
}

function dependencyGraph(packages) {
  const byName = new Map(packages.map((pkg) => [pkg.name, pkg]));
  const reverse = new Map(packages.map((pkg) => [pkg.name, new Set()]));

  for (const pkg of packages) {
    for (const dependency of pkg.dependencies ?? []) {
      const dependencyNames = [dependency.name, dependency.rename].filter(Boolean);
      for (const dependencyName of dependencyNames) {
        if (byName.has(dependencyName)) {
          reverse.get(dependencyName).add(pkg.name);
        }
      }
    }
  }
  return reverse;
}

function reverseDependencyClosure(changedNames, reverse) {
  const closure = new Set(changedNames);
  const queue = [...changedNames];
  for (let index = 0; index < queue.length; index += 1) {
    const packageName = queue[index];
    for (const dependent of reverse.get(packageName) ?? []) {
      if (closure.has(dependent)) continue;
      closure.add(dependent);
      queue.push(dependent);
    }
  }
  return [...closure].sort();
}

/**
 * Classify a Rust PR before scheduling the expensive workspace jobs.
 *
 * A lightweight run is only valid for a same-repository intermediate Stack
 * layer whose Rust changes stay inside workspace member source trees. Any
 * workspace/lockfile/toolchain/deployment change deliberately falls back to
 * the complete Rust suite. The returned package list includes reverse
 * dependents for `cargo check`; tests stay scoped to directly changed crates.
 */
export function classifyRustScope({
  changedFiles,
  metadata,
  repoRoot = process.cwd(),
  stackIntermediate = false,
}) {
  if (!stackIntermediate) {
    return {
      scope: "full",
      packages: [],
      testPackages: [],
      reason: "top-level or non-Stack PR requires full Rust validation",
    };
  }

  const files = [...new Set(changedFiles.map(normalizeFile).filter(Boolean))];
  const rustFiles = files.filter(isRustRelevant);
  if (rustFiles.length === 0) {
    return {
      scope: "full",
      packages: [],
      testPackages: [],
      reason: "no Rust files were available for a safe lightweight scope",
    };
  }

  const packages = workspacePackages(metadata);
  if (packages.length === 0) {
    return {
      scope: "full",
      packages: [],
      testPackages: [],
      reason: "cargo metadata did not describe workspace packages",
    };
  }

  const changedPackages = new Set();
  for (const file of rustFiles) {
    if (isBroadChange(file)) {
      return {
        scope: "full",
        packages: [],
        testPackages: [],
        reason: `broad Rust boundary changed: ${displayFile(file)}`,
      };
    }

    const pkg = packageForFile(file, packages, repoRoot);
    if (!pkg) {
      return {
        scope: "full",
        packages: [],
        testPackages: [],
        reason: `Rust file is outside a known workspace member: ${displayFile(file)}`,
      };
    }
    changedPackages.add(pkg.name);
  }

  if (changedPackages.size === 0) {
    return {
      scope: "full",
      packages: [],
      testPackages: [],
      reason: "Rust changes did not map to a workspace member",
    };
  }

  const reverse = dependencyGraph(packages);
  const testPackages = [...changedPackages].sort();
  const checkPackages = reverseDependencyClosure(testPackages, reverse);
  return {
    scope: "lightweight",
    packages: checkPackages,
    testPackages,
    reason: `intermediate Stack layer; check closure has ${checkPackages.length} package(s)`,
  };
}

function parseArguments(argv) {
  const options = new Map();
  for (let index = 0; index < argv.length; index += 1) {
    const argument = argv[index];
    if (!argument.startsWith("--")) continue;
    const key = argument.slice(2);
    const value = argv[index + 1]?.startsWith("--") ? "" : (argv[++index] ?? "");
    options.set(key, value);
  }
  return options;
}

async function main() {
  const options = parseArguments(process.argv.slice(2));
  const changedFiles = (await readFile(options.get("changed-files"), "utf8")).split(/\r?\n/u);
  const metadata = JSON.parse(await readFile(options.get("metadata"), "utf8"));
  const result = classifyRustScope({
    changedFiles,
    metadata,
    repoRoot: options.get("repo-root") || process.cwd(),
    stackIntermediate: options.get("stack-intermediate") === "true",
  });

  console.log(`rust_scope=${result.scope}`);
  console.log(`rust_packages=${result.packages.join(" ")}`);
  console.log(`rust_test_packages=${result.testPackages.join(" ")}`);
  console.log(`rust_scope_reason=${result.reason}`);
}

const entrypoint = process.argv[1] && path.resolve(process.argv[1]);
if (!process.execArgv.includes("--test") && entrypoint === fileURLToPath(import.meta.url)) {
  await main();
}
