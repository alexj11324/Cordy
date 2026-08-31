#!/usr/bin/env node

import process from "node:process";
import { pathToFileURL } from "node:url";

const TURBO_PREFIX = "turbo-";
const COMMIT_SUFFIX = /-[0-9a-f]{40,64}$/u;

export function selectCacheIdsForDeletion(caches, options) {
  const turboCaches = caches.filter((cache) => cache.key.startsWith(TURBO_PREFIX));

  if (options.mode === "delete-ref") {
    return caches.map((cache) => cache.id);
  }

  if (options.mode === "prune-pr") {
    const groups = new Map();
    for (const cache of turboCaches) {
      const family = cache.key.replace(COMMIT_SUFFIX, "");
      const group = groups.get(family) ?? [];
      group.push(cache);
      groups.set(family, group);
    }
    return [...groups.values()].flatMap((group) =>
      group
        .sort((left, right) =>
          right.created_at.localeCompare(left.created_at),
        )
        .slice(1)
        .map((cache) => cache.id),
    );
  }

  if (options.mode === "prune-main") {
    const groups = new Map();
    for (const cache of turboCaches) {
      const family = cache.key.replace(COMMIT_SUFFIX, "");
      const group = groups.get(family) ?? [];
      group.push(cache);
      groups.set(family, group);
    }

    return [...groups.values()].flatMap((group) =>
      group
        .sort((left, right) =>
          right.created_at.localeCompare(left.created_at),
        )
        .slice(options.keep)
        .map((cache) => cache.id),
    );
  }

  throw new Error(`Unsupported cleanup mode: ${options.mode}`);
}

function parseArguments(argv) {
  const values = new Map();
  for (let index = 0; index < argv.length; index += 2) {
    const key = argv[index];
    const value = argv[index + 1];
    if (!key?.startsWith("--") || value === undefined) {
      throw new Error(`Invalid argument list near ${key ?? "end of input"}`);
    }
    values.set(key.slice(2), value);
  }

  const mode = values.get("mode");
  const ref = values.get("ref");
  const sha = values.get("sha");
  const keep = Number(values.get("keep") ?? "2");
  if (!mode || !ref || !Number.isInteger(keep) || keep < 1) {
    throw new Error("Usage: cleanup-actions-caches.mjs --mode <mode> --ref <ref> [--sha <sha>] [--keep <count>]");
  }
  return { mode, ref, sha, keep };
}

async function githubRequest(path, init = {}) {
  const token = process.env.GITHUB_TOKEN;
  if (!token) throw new Error("GITHUB_TOKEN is required");
  const response = await fetch(`https://api.github.com${path}`, {
    ...init,
    headers: {
      Accept: "application/vnd.github+json",
      Authorization: `Bearer ${token}`,
      "X-GitHub-Api-Version": "2022-11-28",
      ...init.headers,
    },
  });
  if (!response.ok) {
    throw new Error(`GitHub API ${init.method ?? "GET"} ${path} failed: ${response.status} ${await response.text()}`);
  }
  return response.status === 204 ? undefined : response.json();
}

async function listCaches(repository, ref) {
  const caches = [];
  for (let page = 1; ; page += 1) {
    const query = new URLSearchParams({ ref, per_page: "100", page: String(page) });
    const payload = await githubRequest(`/repos/${repository}/actions/caches?${query}`);
    caches.push(...payload.actions_caches);
    if (payload.actions_caches.length < 100) return caches;
  }
}

async function main() {
  const options = parseArguments(process.argv.slice(2));
  const repository = process.env.GITHUB_REPOSITORY;
  if (!repository) throw new Error("GITHUB_REPOSITORY is required");

  const caches = await listCaches(repository, options.ref);
  const ids = new Set(selectCacheIdsForDeletion(caches, options));
  const selected = caches.filter((cache) => ids.has(cache.id));
  for (const cache of selected) {
    await githubRequest(`/repos/${repository}/actions/caches/${cache.id}`, { method: "DELETE" });
  }

  const deletedBytes = selected.reduce((sum, cache) => sum + cache.size_in_bytes, 0);
  const summary = [
    "### GitHub Actions cache maintenance",
    `- Mode: \`${options.mode}\``,
    `- Ref: \`${options.ref}\``,
    `- Entries inspected: \`${caches.length}\``,
    `- Entries deleted: \`${selected.length}\``,
    `- Bytes deleted: \`${deletedBytes}\``,
  ].join("\n");
  console.log(summary);
  if (process.env.GITHUB_STEP_SUMMARY) {
    const { appendFile } = await import("node:fs/promises");
    await appendFile(process.env.GITHUB_STEP_SUMMARY, `${summary}\n`);
  }
}

if (import.meta.url === pathToFileURL(process.argv[1]).href) {
  main().catch((error) => {
    console.error(error instanceof Error ? error.message : error);
    process.exitCode = 1;
  });
}
