import assert from "node:assert/strict";
import test from "node:test";

import {
  compareStableVersions,
  publishHomebrewCask,
} from "./publish-homebrew-cask.mjs";

const cask = (version, marker = "") => `cask "patchbay" do
  version "${version}"
  sha256 "${marker || version}"
end
`;

const encoded = (content) => Buffer.from(content, "utf8").toString("base64");

test("stable version comparison is numeric, not lexical", () => {
  assert.equal(compareStableVersions("0.2.10", "0.2.9"), 1);
  assert.equal(compareStableVersions("1.0.0", "1.0.0"), 0);
  assert.equal(compareStableVersions("0.9.9", "1.0.0"), -1);
  assert.throws(() => compareStableVersions("1.0.0-rc1", "1.0.0"));
});

test("publishes a newer cask and verifies the stored bytes", async () => {
  const current = cask("0.2.8");
  const candidate = cask("0.2.9");
  const calls = [];
  const responses = [
    Response.json({ sha: "old-sha", encoding: "base64", content: encoded(current) }),
    Response.json({ content: { sha: "new-sha" } }),
    Response.json({ sha: "new-sha", encoding: "base64", content: encoded(candidate) }),
  ];
  const fetchImpl = async (url, init = {}) => {
    calls.push({ url: String(url), init });
    return responses.shift();
  };

  await assert.doesNotReject(
    publishHomebrewCask({
      token: "secret",
      tag: "v0.2.9",
      caskContent: candidate,
      fetchImpl,
    }),
  );

  assert.equal(calls.length, 3);
  assert.equal(calls[1].init.method, "PUT");
  const body = JSON.parse(calls[1].init.body);
  assert.equal(body.sha, "old-sha");
  assert.equal(body.branch, "main");
  assert.equal(Buffer.from(body.content, "base64").toString("utf8"), candidate);
});

test("re-reads the tap after a concurrent contents update", async () => {
  const candidate = cask("0.2.10");
  const calls = [];
  const responses = [
    Response.json({
      sha: "stale-sha",
      encoding: "base64",
      content: encoded(cask("0.2.8")),
    }),
    new Response("sha conflict", { status: 409 }),
    Response.json({
      sha: "fresh-sha",
      encoding: "base64",
      content: encoded(cask("0.2.9")),
    }),
    Response.json({ content: { sha: "published-sha" } }),
    Response.json({
      sha: "published-sha",
      encoding: "base64",
      content: encoded(candidate),
    }),
  ];
  const fetchImpl = async (_url, init = {}) => {
    calls.push(init.method ?? "GET");
    return responses.shift();
  };

  const result = await publishHomebrewCask({
    token: "secret",
    tag: "v0.2.10",
    caskContent: candidate,
    fetchImpl,
  });
  assert.deepEqual(result, { status: "updated", version: "0.2.10" });
  assert.deepEqual(calls, ["GET", "PUT", "GET", "PUT", "GET"]);
});

test("accepts a newer cask that wins after this run writes", async () => {
  const candidate = cask("0.2.9");
  const responses = [
    Response.json({
      sha: "old-sha",
      encoding: "base64",
      content: encoded(cask("0.2.8")),
    }),
    Response.json({ content: { sha: "our-sha" } }),
    Response.json({
      sha: "newer-sha",
      encoding: "base64",
      content: encoded(cask("0.3.0")),
    }),
  ];
  const fetchImpl = async () => responses.shift();

  const result = await publishHomebrewCask({
    token: "secret",
    tag: "v0.2.9",
    caskContent: candidate,
    fetchImpl,
  });
  assert.deepEqual(result, { status: "superseded", version: "0.3.0" });
});

test("creates the first cask when the tap has no current file", async () => {
  const candidate = cask("0.2.9");
  const methods = [];
  const responses = [
    new Response("not found", { status: 404 }),
    Response.json({ content: { sha: "new-sha" } }),
    Response.json({ sha: "new-sha", encoding: "base64", content: encoded(candidate) }),
  ];
  const fetchImpl = async (_url, init = {}) => {
    methods.push(init.method ?? "GET");
    return responses.shift();
  };

  await publishHomebrewCask({
    token: "secret",
    tag: "v0.2.9",
    caskContent: candidate,
    fetchImpl,
  });
  assert.deepEqual(methods, ["GET", "PUT", "GET"]);
});

test("refuses to downgrade the stable cask", async () => {
  let calls = 0;
  const fetchImpl = async () => {
    calls += 1;
    return Response.json({
      sha: "newer-sha",
      encoding: "base64",
      content: encoded(cask("0.3.0")),
    });
  };

  await assert.rejects(
    publishHomebrewCask({
      token: "secret",
      tag: "v0.2.9",
      caskContent: cask("0.2.9"),
      fetchImpl,
    }),
    /refusing to replace newer Homebrew cask 0\.3\.0 with 0\.2\.9/u,
  );
  assert.equal(calls, 1);
});

test("an identical current cask is an idempotent no-op", async () => {
  const candidate = cask("0.2.9");
  let calls = 0;
  const fetchImpl = async () => {
    calls += 1;
    return Response.json({
      sha: "same-sha",
      encoding: "base64",
      content: encoded(candidate),
    });
  };

  const result = await publishHomebrewCask({
    token: "secret",
    tag: "v0.2.9",
    caskContent: candidate,
    fetchImpl,
  });
  assert.deepEqual(result, { status: "unchanged", version: "0.2.9" });
  assert.equal(calls, 1);
});

test("refuses same-version content replacement and prerelease tags", async () => {
  const fetchImpl = async () =>
    Response.json({
      sha: "same-version-sha",
      encoding: "base64",
      content: encoded(cask("0.2.9", "old")),
    });

  await assert.rejects(
    publishHomebrewCask({
      token: "secret",
      tag: "v0.2.9",
      caskContent: cask("0.2.9", "different"),
      fetchImpl,
    }),
    /same version has different content/u,
  );
  await assert.rejects(
    publishHomebrewCask({
      token: "secret",
      tag: "v0.3.0-rc1",
      caskContent: cask("0.3.0-rc1"),
      fetchImpl,
    }),
    /stable semantic version/u,
  );
});
