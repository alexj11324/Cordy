import {
  chmodSync,
  copyFileSync,
  existsSync,
  readFileSync,
  statSync,
  writeFileSync,
} from "node:fs";
import { basename, isAbsolute, join, resolve } from "node:path";
import { parseEnv } from "node:util";

import { ensureSecretsFile } from "../../../scripts/ensure-dev-integration-secrets.mjs";
import { offsetForPath } from "./worktree-dev-env.mjs";

const PROCESS_ONLY_CLERK_KEYS = [
  "NEXT_PUBLIC_CLERK_PUBLISHABLE_KEY",
  "CLERK_PUBLISHABLE_KEY",
  "CLERK_SECRET_KEY",
  "CLERK_JWT_KEY",
  "CLERK_ISSUER",
  "CLERK_AUTHORIZED_PARTIES",
];

function isLinkedWorktree(repoRoot) {
  try {
    return statSync(join(repoRoot, ".git")).isFile();
  } catch {
    return false;
  }
}

export function selectDevEnvFile({ repoRoot, env = process.env }) {
  const configured = env.PATCHBAY_DEV_ENV_FILE || env.ENV_FILE;
  if (configured) {
    return isAbsolute(configured) ? configured : resolve(repoRoot, configured);
  }
  return join(repoRoot, isLinkedWorktree(repoRoot) ? ".env.worktree" : ".env");
}

function expandParsedValues(values, inherited = {}) {
  let expanded = { ...values };
  for (let pass = 0; pass < 5; pass += 1) {
    let changed = false;
    expanded = Object.fromEntries(
      Object.entries(expanded).map(([key, value]) => {
        const next = value.replace(
          /\$\{([A-Za-z_][A-Za-z0-9_]*)\}/g,
          (_, name) =>
            Object.hasOwn(expanded, name)
              ? expanded[name]
              : (inherited[name] ?? ""),
        );
        if (next !== value) changed = true;
        return [key, next];
      }),
    );
    if (!changed) break;
  }
  return expanded;
}

export function applyLocalDevEnv(env, { repoRoot } = {}) {
  env.POSTGRES_DB ||= "patchbay";
  env.POSTGRES_USER ||= "patchbay";
  env.POSTGRES_PORT ||= "5432";
  env.PORT =
    env.BACKEND_PORT || env.API_PORT || env.SERVER_PORT || env.PORT || "8080";
  env.FRONTEND_PORT ||= "3000";
  env.FRONTEND_ORIGIN ||= `http://localhost:${env.FRONTEND_PORT}`;
  if (env.PATCHBAY_PUBLIC_URL === undefined) {
    env.PATCHBAY_PUBLIC_URL = `http://localhost:${env.PORT}`;
  }
  env.PATCHBAY_APP_URL ||= env.FRONTEND_ORIGIN;
  env.PATCHBAY_SERVER_URL ||= `ws://localhost:${env.PORT}/ws`;
  env.LOCAL_UPLOAD_BASE_URL ||= `http://localhost:${env.PORT}`;
  const localUploadDir = env.LOCAL_UPLOAD_DIR || "./data/uploads";
  env.LOCAL_UPLOAD_DIR = isAbsolute(localUploadDir)
    ? localUploadDir
    : resolve(repoRoot, "server", localUploadDir);
  env.PLAYWRIGHT_BASE_URL ||= env.FRONTEND_ORIGIN;
  return env;
}

export function loadDevCheckoutEnv({
  repoRoot,
  env = process.env,
  envFile = selectDevEnvFile({ repoRoot, env }),
} = {}) {
  if (!existsSync(envFile)) {
    throw new Error(
      `development env file is missing: ${envFile}. Run \`pnpm dev\` to create it`,
    );
  }
  const explicit = { ...env };
  const parsed = expandParsedValues(
    parseEnv(readFileSync(envFile, "utf8")),
    explicit,
  );
  // Clerk development credentials are process-only. Ignore even legacy
  // checkout-file values so the secure bootstrap can never persist or reload
  // them from .env/.env.worktree.
  for (const key of PROCESS_ONLY_CLERK_KEYS) delete parsed[key];
  Object.assign(env, explicit, parsed, {
    ENV_FILE: envFile,
    PATCHBAY_DEV_ENV_FILE: envFile,
  });
  applyLocalDevEnv(env, { repoRoot });
  return { env, envFile };
}

function worktreeEnvContents(repoRoot) {
  const worktreeName = basename(repoRoot);
  const slug =
    worktreeName
      .toLowerCase()
      .replace(/[^a-z0-9]+/g, "_")
      .replace(/^_+|_+$/g, "") || "patchbay";
  const offset = offsetForPath(repoRoot);
  const postgresDb = `patchbay_${slug}_${offset}`;
  const backendPort = 18080 + offset;
  const frontendPort = 13000 + offset;
  const frontendOrigin = `http://localhost:${frontendPort}`;
  return `POSTGRES_DB=${postgresDb}
POSTGRES_USER=patchbay
POSTGRES_PASSWORD=patchbay
POSTGRES_PORT=5432
DATABASE_URL=postgres://patchbay:patchbay@localhost:5432/${postgresDb}?sslmode=disable

PORT=${backendPort}
JWT_SECRET=change-me-in-production
PATCHBAY_DEV_VERIFICATION_CODE=888888
PATCHBAY_SERVER_URL=ws://localhost:${backendPort}/ws
PATCHBAY_PUBLIC_URL=http://localhost:${backendPort}
PATCHBAY_APP_URL=${frontendOrigin}

FRONTEND_PORT=${frontendPort}
FRONTEND_ORIGIN=${frontendOrigin}
NEXT_PUBLIC_API_URL=http://localhost:${backendPort}
NEXT_PUBLIC_WS_URL=ws://localhost:${backendPort}/ws
`;
}

export async function ensureDevCheckoutEnv({
  repoRoot,
  env = process.env,
  log = console,
} = {}) {
  const envFile = selectDevEnvFile({ repoRoot, env });
  if (!existsSync(envFile)) {
    if (isLinkedWorktree(repoRoot)) {
      writeFileSync(envFile, worktreeEnvContents(repoRoot), { mode: 0o600 });
      log.log(`[dev] generated isolated worktree environment ${envFile}`);
    } else {
      copyFileSync(join(repoRoot, ".env.example"), envFile);
      chmodSync(envFile, 0o600);
      log.log(`[dev] created ${envFile} from .env.example`);
    }
  }
  const secrets = await ensureSecretsFile(envFile);
  if (secrets.generated.length > 0) {
    log.log(
      `[dev] generated local-only integration encryption keys in ${envFile}: ${secrets.generated.join(", ")}`,
    );
  }
  return loadDevCheckoutEnv({ repoRoot, env, envFile });
}
