import { pathToFileURL } from "node:url";
import { requireHealthyResponse } from "./verify-production-deployment.mjs";

const SHA_PATTERN = /^[0-9a-f]{40}$/u;
const DEFAULT_ATTEMPTS = 18;
const DEFAULT_DELAY_MS = 5_000;

function sleep(milliseconds) {
  return new Promise((resolve) => setTimeout(resolve, milliseconds));
}

async function request(fetchImpl, url) {
  return fetchImpl(url, {
    redirect: "manual",
    signal: AbortSignal.timeout(10_000),
    headers: { "user-agent": "patchbay-staging-verifier/1" },
  });
}

export async function verifyStagingOnce(sourceSha, fetchImpl = fetch) {
  if (!SHA_PATTERN.test(sourceSha)) {
    throw new Error("source SHA must be 40 lowercase hexadecimal characters");
  }
  const expectedBuild = `sha-${sourceSha}`;

  const configUrl = "https://api.staging.aspectlylabs.com/api/config";
  const configResponse = await request(fetchImpl, configUrl);
  requireHealthyResponse(configResponse, {
    url: configUrl,
    expectedBuild,
    expectedCommit: sourceSha,
    exactStatus: 200,
  });

  for (const url of [
    "https://staging.aspectlylabs.com/login",
    "https://staging.aspectlylabs.com/docs",
  ]) {
    const response = await request(fetchImpl, url);
    requireHealthyResponse(response, {
      url,
      expectedBuild,
      expectedCommit: sourceSha,
      exactStatus: 200,
    });
  }

  const brokerReadyUrl = "https://accounts.staging.aspectlylabs.com/readyz";
  const brokerReadyResponse = await request(fetchImpl, brokerReadyUrl);
  requireHealthyResponse(brokerReadyResponse, {
    url: brokerReadyUrl,
    expectedBuild,
    expectedCommit: sourceSha,
    exactStatus: 200,
  });
}

export async function verifyStagingDeployment(
  sourceSha,
  {
    fetchImpl = fetch,
    attempts = DEFAULT_ATTEMPTS,
    delayMs = DEFAULT_DELAY_MS,
  } = {},
) {
  let lastError;
  for (let attempt = 1; attempt <= attempts; attempt += 1) {
    try {
      await verifyStagingOnce(sourceSha, fetchImpl);
      return;
    } catch (error) {
      lastError = error;
      console.error(
        `staging verification attempt ${attempt}/${attempts} failed: ${error instanceof Error ? error.message : error}`,
      );
      if (attempt < attempts) await sleep(delayMs);
    }
  }
  throw lastError;
}

async function main() {
  const [sourceSha] = process.argv.slice(2);
  if (!sourceSha) {
    throw new Error("usage: verify-staging-deployment.mjs <source-sha>");
  }
  await verifyStagingDeployment(sourceSha);
}

if (import.meta.url === pathToFileURL(process.argv[1] ?? "").href) {
  main().catch((error) => {
    console.error(error instanceof Error ? error.message : error);
    process.exitCode = 1;
  });
}
