import assert from "node:assert/strict";
import test from "node:test";

import { selectCacheIdsForDeletion } from "./cleanup-actions-caches.mjs";

const cache = (id, key, createdAt) => ({
  id,
  key,
  created_at: createdAt,
  size_in_bytes: 1,
});

test("a PR keeps only the newest Turbo entry in each cache family", () => {
  const current = "a".repeat(40);
  const previous = "b".repeat(40);
  const caches = [
    cache(1, `turbo-build-linux-${current}`, "2026-08-31T03:00:00Z"),
    cache(2, `turbo-build-linux-${previous}`, "2026-08-31T02:00:00Z"),
    cache(3, "cargo-downloads-linux-lock", "2026-08-31T01:00:00Z"),
  ];

  assert.deepEqual(
    selectCacheIdsForDeletion(caches, { mode: "prune-pr" }),
    [2],
  );
});

test("main keeps two generations per Turbo cache family", () => {
  const caches = [
    cache(1, `turbo-build-linux-${"a".repeat(40)}`, "2026-08-31T03:00:00Z"),
    cache(2, `turbo-build-linux-${"b".repeat(40)}`, "2026-08-31T02:00:00Z"),
    cache(3, `turbo-build-linux-${"c".repeat(40)}`, "2026-08-31T01:00:00Z"),
    cache(4, `turbo-test-linux-${"d".repeat(40)}`, "2026-08-31T03:00:00Z"),
  ];

  assert.deepEqual(
    selectCacheIdsForDeletion(caches, { mode: "prune-main", keep: 2 }),
    [3],
  );
});

test("a closed PR deletes every cache scoped to its merge ref", () => {
  const caches = [
    cache(1, "turbo-build-anything", "2026-08-31T03:00:00Z"),
    cache(2, "setup-node-pnpm-anything", "2026-08-31T02:00:00Z"),
    cache(3, "sccache-anything", "2026-08-31T01:00:00Z"),
  ];

  assert.deepEqual(
    selectCacheIdsForDeletion(caches, { mode: "delete-ref" }),
    [1, 2, 3],
  );
});
