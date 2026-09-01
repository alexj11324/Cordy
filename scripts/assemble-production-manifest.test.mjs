import assert from "node:assert/strict";
import { mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import test from "node:test";

import { assembleProductionManifest } from "./assemble-production-manifest.mjs";

const SOURCE_SHA = "a".repeat(40);
const DIGEST = `sha256:${"b".repeat(64)}`;
const NAMES = ["backend", "web", "docs", "auth-broker"];

async function fixture() {
  const directory = await mkdtemp(
    path.join(os.tmpdir(), "production-manifest-"),
  );
  for (const name of NAMES) {
    const repository = `ghcr.io/alexj11324/patchbay-${name}`;
    await writeFile(
      path.join(directory, `${name}.json`),
      JSON.stringify({
        schema_version: 1,
        name,
        repository,
        digest: DIGEST,
        source_sha: SOURCE_SHA,
        ref: `${repository}@${DIGEST}`,
      }),
    );
  }
  return directory;
}

test("assembles one immutable manifest from the complete image set", async (t) => {
  const directory = await fixture();
  t.after(() => rm(directory, { recursive: true, force: true }));
  const outputPath = path.join(
    directory,
    "..",
    `${path.basename(directory)}.json`,
  );
  t.after(() => rm(outputPath, { force: true }));

  const manifest = await assembleProductionManifest({
    inputDirectory: directory,
    sourceSha: SOURCE_SHA,
    outputPath,
    repositoryOwner: "alexj11324",
    runId: "123",
  });

  assert.deepEqual(Object.keys(manifest.images), NAMES);
  assert.equal(manifest.source_sha, SOURCE_SHA);
  assert.equal(manifest.workflow_run_id, "123");
  assert.deepEqual(JSON.parse(await readFile(outputPath, "utf8")), manifest);
});

test("fails closed when any production image is missing", async (t) => {
  const directory = await fixture();
  t.after(() => rm(directory, { recursive: true, force: true }));
  await rm(path.join(directory, "docs.json"));

  await assert.rejects(
    assembleProductionManifest({
      inputDirectory: directory,
      sourceSha: SOURCE_SHA,
      outputPath: path.join(directory, "manifest.json"),
      repositoryOwner: "alexj11324",
    }),
    /expected 4 image records, found 3/u,
  );
});

test("rejects mixed source commits", async (t) => {
  const directory = await fixture();
  t.after(() => rm(directory, { recursive: true, force: true }));
  const docsPath = path.join(directory, "docs.json");
  const docs = JSON.parse(await readFile(docsPath, "utf8"));
  docs.source_sha = "c".repeat(40);
  await writeFile(docsPath, JSON.stringify(docs));

  await assert.rejects(
    assembleProductionManifest({
      inputDirectory: directory,
      sourceSha: SOURCE_SHA,
      outputPath: path.join(directory, "manifest.json"),
      repositoryOwner: "alexj11324",
    }),
    /was built from/u,
  );
});
