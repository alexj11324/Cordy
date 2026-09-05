import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

const hosted = JSON.parse(
  await readFile(new URL("../deploy/origin/hosted-environments.json", import.meta.url), "utf8"),
);

async function read(path) {
  return readFile(new URL(`../${path}`, import.meta.url), "utf8");
}

test("hosted environments keep development, staging, and production distinct", () => {
  const { development, staging, production } = hosted.environments;
  assert.equal(development.public, false);
  assert.equal(staging.public, false);
  assert.equal(production.public, true);
  assert.equal(development.official_cloud, false);
  assert.equal(staging.official_cloud, false);
  assert.equal(production.official_cloud, true);

  const apiUrls = new Set([development.api_url, staging.api_url, production.api_url]);
  const appUrls = new Set([development.app_url, staging.app_url, production.app_url]);
  assert.equal(apiUrls.size, 3);
  assert.equal(appUrls.size, 3);
  assert.notEqual(staging.ports.backend, production.ports.backend);
  assert.notEqual(staging.ports.web, production.ports.web);
  assert.notEqual(staging.ports.docs, production.ports.docs);
  assert.notEqual(staging.ports.auth_broker, production.ports.auth_broker);
  assert.notEqual(staging.compose_project, production.compose_project);
  assert.notEqual(staging.state_root, production.state_root);
  assert.equal(staging.github_environment, "staging");
  assert.equal(production.github_environment, "production");
});

test("client env files match the hosted staging contract and leave copilothub", async () => {
  const desktop = await read("apps/desktop/.env.staging");
  const mobile = await read("apps/mobile/.env.staging");
  const mobileProd = await read("apps/mobile/.env.production");
  const { staging, production } = hosted.environments;

  assert.match(desktop, new RegExp(`VITE_API_URL=${staging.api_url}`));
  assert.match(desktop, new RegExp(`VITE_APP_URL=${staging.app_url}`));
  assert.match(desktop, new RegExp(`VITE_ACCOUNTS_URL=${staging.accounts_url}`));
  assert.match(mobile, new RegExp(`EXPO_PUBLIC_API_URL=${staging.api_url}`));
  assert.match(mobile, new RegExp(`EXPO_PUBLIC_WEB_URL=${staging.app_url}`));
  assert.match(mobileProd, new RegExp(`EXPO_PUBLIC_API_URL=${production.api_url}`));
  assert.match(mobileProd, new RegExp(`EXPO_PUBLIC_WEB_URL=${production.app_url}`));
  for (const source of [desktop, mobile, mobileProd]) {
    assert.doesNotMatch(source, /copilothub/i);
  }
});

test("desktop default runtime stays on public production", async () => {
  const runtime = await read("apps/desktop/src/shared/runtime-config.ts");
  const { production } = hosted.environments;
  assert.match(runtime, new RegExp(`apiUrl: "${production.api_url}"`));
  assert.match(runtime, new RegExp(`appUrl: "${production.app_url}"`));
  assert.match(runtime, new RegExp(`accountsUrl: "${production.accounts_url}"`));
});
