import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

const root = new URL("../", import.meta.url);

async function read(path) {
  return readFile(new URL(path, root), "utf8");
}

test("release publishes the CLI as a cask in the dedicated Homebrew tap", async () => {
  const [config, workflow] = await Promise.all([
    read(".goreleaser.yml"),
    read(".github/workflows/release.yml"),
  ]);

  assert.match(config, /^homebrew_casks:$/mu);
  assert.doesNotMatch(config, /^brews:$/mu);
  assert.match(config, /^\s+name: homebrew-tap$/mu);
  assert.match(config, /^\s+directory: Casks$/mu);
  assert.match(config, /^\s+binaries:\n\s+- patchbay$/mu);
  assert.match(
    config,
    /token: "\{\{ \.Env\.HOMEBREW_TAP_GITHUB_TOKEN \}\}"/u,
  );

  assert.match(
    workflow,
    /HOMEBREW_TAP_GITHUB_TOKEN: \$\{\{ secrets\.HOMEBREW_TAP_GITHUB_TOKEN \}\}/u,
  );
  assert.match(
    workflow,
    /HOMEBREW_TAP_GITHUB_TOKEN is required to publish alexj11324\/homebrew-tap/u,
  );
  assert.match(
    workflow,
    /node --test scripts\/release-homebrew-contract\.test\.mjs/u,
  );
});

test("all supported Homebrew instructions use the dedicated tap", async () => {
  const surfaces = [
    "CLI_INSTALL.md",
    "CLI_AND_DAEMON.md",
    "SELF_HOSTING.md",
    "SELF_HOSTING_AI.md",
    "scripts/install.sh",
    "scripts/selfhost-wait.sh",
    "apps/docs/content/docs/cli.mdx",
    "apps/docs/content/docs/cli.ja.mdx",
    "apps/docs/content/docs/cli.ko.mdx",
    "apps/docs/content/docs/cli.zh.mdx",
  ];

  for (const surface of surfaces) {
    const content = await read(surface);
    assert.match(
      content,
      /alexj11324\/tap\/patchbay/u,
      `${surface} must point to the dedicated Homebrew tap`,
    );
    assert.doesNotMatch(content, /alexj11324\/Cordy\/patchbay/u);
    assert.doesNotMatch(content, /brew tap alexj11324\/Cordy/u);
  }
});
