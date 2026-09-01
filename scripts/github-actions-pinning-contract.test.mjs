import assert from "node:assert/strict";
import { existsSync, readdirSync, readFileSync } from "node:fs";
import { dirname, extname, join, relative } from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

const repoRoot = join(dirname(fileURLToPath(import.meta.url)), "..");
const actionRoots = [
  join(repoRoot, ".github", "workflows"),
  join(repoRoot, ".github", "actions"),
];

function yamlFiles(root) {
  if (!existsSync(root)) return [];

  return readdirSync(root, { withFileTypes: true }).flatMap((entry) => {
    const path = join(root, entry.name);
    if (entry.isDirectory()) return yamlFiles(path);
    return [".yml", ".yaml"].includes(extname(entry.name)) ? [path] : [];
  });
}

function isImmutableUsesReference(reference) {
  if (reference.startsWith("./")) return true;
  if (reference.startsWith("docker://")) {
    return /@sha256:[0-9a-f]{64}$/i.test(reference);
  }
  return /^[^/@\s]+\/[^/@\s]+(?:\/[^@\s]+)?@[0-9a-f]{40}$/i.test(reference);
}

function usesReferences(path) {
  return readFileSync(path, "utf8")
    .split("\n")
    .flatMap((line, index) => {
      const match = line.match(/^\s*(?:-\s*)?uses:\s*(.*?)\s*$/);
      if (!match) return [];

      const commentIndex = match[1].search(/\s+#\s*/);
      const rawReference = (
        commentIndex === -1 ? match[1] : match[1].slice(0, commentIndex)
      ).trim();
      const quoted =
        rawReference.length >= 2 &&
        ["'", '"'].includes(rawReference[0]) &&
        rawReference.at(-1) === rawReference[0];
      const reference = quoted ? rawReference.slice(1, -1) : rawReference;
      const versionComment =
        commentIndex === -1
          ? undefined
          : match[1].slice(commentIndex).replace(/^\s+#\s*/, "").trim();

      return [{ line: index + 1, reference, versionComment }];
    });
}

test("rejects mutable external action references", () => {
  assert.equal(isImmutableUsesReference("actions/checkout@v6"), false);
  assert.equal(
    isImmutableUsesReference("docker://rhysd/actionlint:1.7.7"),
    false,
  );
  assert.equal(isImmutableUsesReference("${{ inputs.action }}"), false);
  assert.equal(isImmutableUsesReference("./.github/actions/local"), true);
});

test("all external workflow and action references are immutable and documented", () => {
  const failures = [];

  for (const path of actionRoots.flatMap(yamlFiles)) {
    for (const { line, reference, versionComment } of usesReferences(path)) {
      if (reference.startsWith("./")) continue;
      if (!isImmutableUsesReference(reference)) {
        failures.push(
          `${relative(repoRoot, path)}:${line} has mutable uses reference ${reference}`,
        );
      } else if (!versionComment) {
        failures.push(
          `${relative(repoRoot, path)}:${line} is missing its human-readable version comment`,
        );
      }
    }
  }

  assert.deepEqual(failures, []);
});
