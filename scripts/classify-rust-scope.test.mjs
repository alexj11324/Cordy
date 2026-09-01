import assert from "node:assert/strict";
import test from "node:test";

import { classifyRustScope } from "./classify-rust-scope.mjs";

const metadata = {
  workspace_members: ["core-id", "api-id", "cli-id"],
  packages: [
    {
      id: "core-id",
      name: "patchbay-core",
      manifest_path: "server-rs/crates/core/Cargo.toml",
      dependencies: [],
    },
    {
      id: "api-id",
      name: "patchbay-api",
      manifest_path: "server-rs/crates/api/Cargo.toml",
      dependencies: [{ name: "patchbay-core" }],
    },
    {
      id: "cli-id",
      name: "patchbay-cli",
      manifest_path: "server-rs/crates/cli/Cargo.toml",
      dependencies: [{ name: "patchbay-api" }],
    },
  ],
};

test("intermediate crate changes check reverse workspace dependents", () => {
  const result = classifyRustScope({
    changedFiles: ["server-rs/crates/core/src/lib.rs"],
    metadata,
    repoRoot: "/repo",
    stackIntermediate: true,
  });

  assert.equal(result.scope, "lightweight");
  assert.deepEqual(result.testPackages, ["patchbay-core"]);
  assert.deepEqual(result.packages, ["patchbay-api", "patchbay-cli", "patchbay-core"]);
});

test("member manifest changes use the full suite", () => {
  const result = classifyRustScope({
    changedFiles: ["server-rs/crates/core/Cargo.toml"],
    metadata,
    stackIntermediate: true,
  });

  assert.equal(result.scope, "full");
  assert.match(result.reason, /broad Rust boundary/u);
});

test("workspace and migration boundaries use the full suite", () => {
  for (const changedFile of ["server-rs/Cargo.lock", "server-rs/Cargo.toml", "migrations/20260901_init.sql"]) {
    const result = classifyRustScope({
      changedFiles: [changedFile],
      metadata,
      stackIntermediate: true,
    });
    assert.equal(result.scope, "full", changedFile);
  }
});

test("a top-level PR remains a full Rust validation even for a narrow crate diff", () => {
  const result = classifyRustScope({
    changedFiles: ["server-rs/crates/core/src/lib.rs"],
    metadata,
    stackIntermediate: false,
  });

  assert.equal(result.scope, "full");
  assert.match(result.reason, /top-level/u);
});

test("unknown Rust paths fail closed to the full suite", () => {
  const result = classifyRustScope({
    changedFiles: ["server-rs/unknown-generated-file.rs"],
    metadata,
    stackIntermediate: true,
  });

  assert.equal(result.scope, "full");
  assert.match(result.reason, /broad|outside/u);
});
