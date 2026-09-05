import assert from "node:assert/strict";
import { mkdtemp, readFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";
import test from "node:test";
import {
  parseInspectDigest,
  writePublishedImageRecords,
} from "./resolve-published-image-records.mjs";

test("parseInspectDigest reads the image digest line", () => {
  assert.equal(
    parseInspectDigest("Name: ghcr.io/example/app:sha-abc\nDigest: sha256:" + "a".repeat(64) + "\n"),
    "sha256:" + "a".repeat(64),
  );
});

test("writePublishedImageRecords emits the production assembler input shape", async () => {
  const directory = await mkdtemp(path.join(tmpdir(), "patchbay-image-records-"));
  const sourceSha = "b".repeat(40);
  const digest = "sha256:" + "c".repeat(64);
  await writePublishedImageRecords({
    outputDirectory: directory,
    sourceSha,
    repositoryOwner: "alexj11324",
    inspect: (reference) => {
      assert.match(reference, /^ghcr\.io\/alexj11324\/patchbay-(backend|web|docs|auth-broker):sha-b{40}$/u);
      return `Digest: ${digest}\n`;
    },
  });
  const backend = JSON.parse(await readFile(path.join(directory, "backend.json"), "utf8"));
  assert.deepEqual(backend, {
    schema_version: 1,
    name: "backend",
    repository: "ghcr.io/alexj11324/patchbay-backend",
    digest,
    source_sha: sourceSha,
    ref: `ghcr.io/alexj11324/patchbay-backend@${digest}`,
  });
});
