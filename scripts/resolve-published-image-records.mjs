import { execFileSync } from "node:child_process";
import { mkdir, writeFile } from "node:fs/promises";
import path from "node:path";
import { pathToFileURL } from "node:url";

const REQUIRED_IMAGES = ["backend", "web", "docs", "auth-broker"];
const SHA_PATTERN = /^[0-9a-f]{40}$/u;
const DIGEST_PATTERN = /^sha256:[0-9a-f]{64}$/u;

export function parseInspectDigest(output) {
  const match = String(output).match(/^Digest:\s+(sha256:[0-9a-f]{64})\s*$/m);
  if (!match) {
    throw new Error("imagetools inspect output is missing a sha256 digest");
  }
  return match[1];
}

export async function writePublishedImageRecords({
  outputDirectory,
  sourceSha,
  repositoryOwner,
  inspect = inspectImage,
}) {
  if (!SHA_PATTERN.test(sourceSha)) {
    throw new Error(`source SHA must be 40 lowercase hexadecimal characters: ${sourceSha}`);
  }
  if (!/^[A-Za-z0-9_.-]+$/u.test(repositoryOwner)) {
    throw new Error(`invalid repository owner: ${repositoryOwner}`);
  }
  await mkdir(outputDirectory, { recursive: true });
  for (const name of REQUIRED_IMAGES) {
    const repository = `ghcr.io/${repositoryOwner}/patchbay-${name}`;
    const digest = parseInspectDigest(inspect(`${repository}:sha-${sourceSha}`));
    if (!DIGEST_PATTERN.test(digest)) {
      throw new Error(`invalid ${name} digest: ${digest}`);
    }
    const record = {
      schema_version: 1,
      name,
      repository,
      digest,
      source_sha: sourceSha,
      ref: `${repository}@${digest}`,
    };
    await writeFile(
      path.join(outputDirectory, `${name}.json`),
      `${JSON.stringify(record, null, 2)}\n`,
      "utf8",
    );
  }
}

function inspectImage(reference) {
  return execFileSync("docker", ["buildx", "imagetools", "inspect", reference], {
    encoding: "utf8",
  });
}

async function main() {
  const [outputDirectory, sourceSha, repositoryOwner] = process.argv.slice(2);
  if (!outputDirectory || !sourceSha || !repositoryOwner) {
    throw new Error(
      "usage: resolve-published-image-records.mjs <output-directory> <source-sha> <repository-owner>",
    );
  }
  await writePublishedImageRecords({
    outputDirectory,
    sourceSha,
    repositoryOwner,
  });
}

if (import.meta.url === pathToFileURL(process.argv[1] ?? "").href) {
  main().catch((error) => {
    console.error(error instanceof Error ? error.message : error);
    process.exitCode = 1;
  });
}
