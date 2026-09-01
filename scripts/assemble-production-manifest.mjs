import { readFile, readdir, writeFile } from "node:fs/promises";
import path from "node:path";
import { pathToFileURL } from "node:url";

const REQUIRED_IMAGES = ["backend", "web", "docs", "auth-broker"];
const SHA_PATTERN = /^[0-9a-f]{40}$/u;
const DIGEST_PATTERN = /^sha256:[0-9a-f]{64}$/u;

function requireString(value, label) {
  if (typeof value !== "string" || value.length === 0) {
    throw new Error(`${label} must be a non-empty string`);
  }
  return value;
}

export async function assembleProductionManifest({
  inputDirectory,
  sourceSha,
  outputPath,
  repositoryOwner,
  repository = `${repositoryOwner}/Cordy`, // legacy-brand-compat: live repository identity
  runId = process.env.GITHUB_RUN_ID ?? "local",
}) {
  if (!SHA_PATTERN.test(sourceSha)) {
    throw new Error(
      `source SHA must be 40 lowercase hexadecimal characters: ${sourceSha}`,
    );
  }
  if (!/^[A-Za-z0-9_.-]+$/u.test(repositoryOwner)) {
    throw new Error(`invalid repository owner: ${repositoryOwner}`);
  }

  const files = (await readdir(inputDirectory))
    .filter((entry) => entry.endsWith(".json"))
    .sort();
  if (files.length !== REQUIRED_IMAGES.length) {
    throw new Error(
      `expected ${REQUIRED_IMAGES.length} image records, found ${files.length}: ${files.join(", ")}`,
    );
  }

  const records = new Map();
  for (const file of files) {
    const raw = await readFile(path.join(inputDirectory, file), "utf8");
    const record = JSON.parse(raw);
    const name = requireString(record.name, `${file} name`);
    if (!REQUIRED_IMAGES.includes(name)) {
      throw new Error(`${file} has unexpected image name: ${name}`);
    }
    if (records.has(name)) {
      throw new Error(`duplicate image record: ${name}`);
    }
    if (record.schema_version !== 1) {
      throw new Error(`${file} has unsupported schema version`);
    }
    if (record.source_sha !== sourceSha) {
      throw new Error(
        `${file} was built from ${record.source_sha}, expected ${sourceSha}`,
      );
    }
    const digest = requireString(record.digest, `${file} digest`);
    if (!DIGEST_PATTERN.test(digest)) {
      throw new Error(`${file} has invalid digest: ${digest}`);
    }
    const expectedRepository = `ghcr.io/${repositoryOwner}/patchbay-${name}`;
    if (record.repository !== expectedRepository) {
      throw new Error(
        `${file} has repository ${record.repository}, expected ${expectedRepository}`,
      );
    }
    const expectedRef = `${expectedRepository}@${digest}`;
    if (record.ref !== expectedRef) {
      throw new Error(`${file} has ref ${record.ref}, expected ${expectedRef}`);
    }
    records.set(name, expectedRef);
  }

  for (const name of REQUIRED_IMAGES) {
    if (!records.has(name)) {
      throw new Error(`missing required production image: ${name}`);
    }
  }

  const manifest = {
    schema_version: 1,
    action: "deploy",
    repository,
    source_sha: sourceSha,
    workflow_run_id: String(runId),
    images: Object.fromEntries(
      REQUIRED_IMAGES.map((name) => [name, records.get(name)]),
    ),
  };
  await writeFile(outputPath, `${JSON.stringify(manifest, null, 2)}\n`, "utf8");
  return manifest;
}

async function main() {
  const [inputDirectory, sourceSha, outputPath, repositoryOwner] =
    process.argv.slice(2);
  if (!inputDirectory || !sourceSha || !outputPath || !repositoryOwner) {
    throw new Error(
      "usage: assemble-production-manifest.mjs <input-directory> <source-sha> <output-path> <repository-owner>",
    );
  }
  await assembleProductionManifest({
    inputDirectory,
    sourceSha,
    outputPath,
    repositoryOwner,
  });
}

if (import.meta.url === pathToFileURL(process.argv[1] ?? "").href) {
  main().catch((error) => {
    console.error(error instanceof Error ? error.message : error);
    process.exitCode = 1;
  });
}
