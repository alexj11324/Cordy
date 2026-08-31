import { pathToFileURL } from "node:url";

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
    headers: { "user-agent": "patchbay-production-verifier/1" },
  });
}

export function requireHealthyResponse(
  response,
  { url, expectedBuild, exactStatus },
) {
  if (exactStatus !== undefined && response.status !== exactStatus) {
    throw new Error(
      `${url} returned HTTP ${response.status}, expected ${exactStatus}`,
    );
  }
  if (response.status < 200 || response.status >= 400) {
    throw new Error(`${url} returned unacceptable HTTP ${response.status}`);
  }
  if (expectedBuild !== undefined) {
    const actual = response.headers.get("x-patchbay-build");
    if (actual !== expectedBuild) {
      throw new Error(
        `${url} reported build ${actual ?? "<missing>"}, expected ${expectedBuild}`,
      );
    }
  }
}

export async function verifyProductionOnce(sourceSha, fetchImpl = fetch) {
  if (!SHA_PATTERN.test(sourceSha)) {
    throw new Error("source SHA must be 40 lowercase hexadecimal characters");
  }
  const expectedBuild = `sha-${sourceSha}`;

  const configUrl = "https://api.aspectlylabs.com/api/config";
  const configResponse = await request(fetchImpl, configUrl);
  requireHealthyResponse(configResponse, { url: configUrl, exactStatus: 200 });
  const config = await configResponse.json();
  if (config.server_version !== expectedBuild) {
    throw new Error(
      `${configUrl} reported server version ${config.server_version ?? "<missing>"}, expected ${expectedBuild}`,
    );
  }

  for (const url of [
    "https://patchbay.aspectlylabs.com/login",
    "https://patchbay.aspectlylabs.com/docs",
  ]) {
    const response = await request(fetchImpl, url);
    requireHealthyResponse(response, { url, expectedBuild, exactStatus: 200 });
  }

  for (const [url, exactStatus] of [
    ["https://accounts.aspectlylabs.com/readyz", 200],
    ["https://accounts.aspectlylabs.com/oauth/google", undefined],
  ]) {
    const response = await request(fetchImpl, url);
    requireHealthyResponse(response, { url, expectedBuild, exactStatus });
  }
}

export async function verifyProductionDeployment(
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
      await verifyProductionOnce(sourceSha, fetchImpl);
      return;
    } catch (error) {
      lastError = error;
      console.error(
        `production verification attempt ${attempt}/${attempts} failed: ${error instanceof Error ? error.message : error}`,
      );
      if (attempt < attempts) await sleep(delayMs);
    }
  }
  throw lastError;
}

async function main() {
  const [sourceSha] = process.argv.slice(2);
  if (!sourceSha) {
    throw new Error("usage: verify-production-deployment.mjs <source-sha>");
  }
  await verifyProductionDeployment(sourceSha);
}

if (import.meta.url === pathToFileURL(process.argv[1] ?? "").href) {
  main().catch((error) => {
    console.error(error instanceof Error ? error.message : error);
    process.exitCode = 1;
  });
}
