import {
  existsSync,
  mkdirSync,
  readFileSync,
  renameSync,
  statSync,
  unlinkSync,
  writeFileSync,
} from "node:fs";
import { execFileSync } from "node:child_process";
import { randomUUID } from "node:crypto";
import { createServer } from "node:net";
import { homedir, tmpdir } from "node:os";
import { basename, dirname, isAbsolute, join, resolve } from "node:path";
import { parseEnv } from "node:util";

import { ensureSecretsFile } from "../../../scripts/ensure-dev-integration-secrets.mjs";
import {
  appSuffixForOffset,
  checkoutIdentity,
  offsetForPath,
  rendererPortForOffset,
} from "./worktree-dev-env.mjs";

const PROCESS_ONLY_CLERK_KEYS = [
  "NEXT_PUBLIC_CLERK_PUBLISHABLE_KEY",
  "CLERK_PUBLISHABLE_KEY",
  "CLERK_SECRET_KEY",
  "CLERK_JWT_KEY",
  "CLERK_ISSUER",
  "CLERK_AUTHORIZED_PARTIES",
];

export const DEV_CHECKOUT_ENV_SCHEMA = "3";
export const DEV_PORT_RESERVATION_LOCK_FILE = join(
  tmpdir(),
  "patchbay-dev-port-reservation.lock",
);
export const DEV_PORT_RESERVATION_REGISTRY_FILE = join(
  homedir(),
  ".patchbay",
  "dev",
  "patchbay-dev-port-reservations.json",
);
const DEV_PORT_RESERVATION_LOCK_WAIT_MS = 30_000;
const DEV_PORT_RESERVATION_LOCK_STALE_MS = 5 * 60 * 1000;
const DEV_PORT_RESERVATION_REGISTRY_SCHEMA = 1;

function checkoutSlug(worktreeName) {
  return (
    worktreeName
      .toLowerCase()
      .replace(/[^a-z0-9]+/g, "_")
      .replace(/^_+|_+$/g, "")
      .slice(0, 40)
      .replace(/_+$/g, "") || "patchbay"
  );
}

export function selectDevEnvFile({ repoRoot, env = process.env }) {
  // ENV_FILE is exported by Make from its own generic .env selection, so it
  // is not proof that a developer intentionally overrode complete-dev
  // isolation. PATCHBAY_DEV_ENV_FILE is the single explicit override.
  const configured = env.PATCHBAY_DEV_ENV_FILE;
  if (configured) {
    return isAbsolute(configured) ? configured : resolve(repoRoot, configured);
  }
  // A linked worktree and an independent clone are indistinguishable by a
  // portable Git query: both consider themselves the primary checkout of
  // their own common Git directory. Keep every source-development checkout
  // isolated instead of letting an independent clone silently fall back to
  // the generic .env database and ports (patchbay / 8080 / 3000).
  return join(repoRoot, ".env.worktree");
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
  env.APP_ENV = "development";
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

function portsForOffset(offset) {
  return [
    18080 + offset,
    13000 + offset,
    rendererPortForOffset(offset),
  ];
}

function readPortReservationRegistry(
  registryPath = DEV_PORT_RESERVATION_REGISTRY_FILE,
) {
  let raw;
  try {
    raw = readFileSync(registryPath, "utf8");
  } catch (error) {
    if (error?.code === "ENOENT") return [];
    throw new Error(
      `could not read the development port reservation registry ${registryPath}: ${error instanceof Error ? error.message : String(error)}`,
    );
  }
  let parsed;
  try {
    parsed = JSON.parse(raw);
  } catch (error) {
    throw new Error(
      `development port reservation registry ${registryPath} is corrupt; remove it only after confirming no other worktree is running (${error instanceof Error ? error.message : String(error)})`,
    );
  }
  if (
    parsed?.schema !== DEV_PORT_RESERVATION_REGISTRY_SCHEMA ||
    !Array.isArray(parsed.reservations)
  ) {
    throw new Error(
      `development port reservation registry ${registryPath} has an unsupported schema; regenerate the affected worktree environment after confirming no other worktree is running`,
    );
  }
  return parsed.reservations.flatMap((entry, index) => {
    if (
      !entry ||
      typeof entry.repoRoot !== "string" ||
      typeof entry.envFile !== "string" ||
      !Number.isInteger(entry.offset) ||
      entry.offset < 0 ||
      entry.offset >= 1000
    ) {
      throw new Error(
        `development port reservation registry ${registryPath} contains an invalid reservation at index ${index}; regenerate it only after confirming no other worktree is running`,
      );
    }
    // Entries written by older schema-1 launchers did not persist the tuple;
    // derive it from the validated offset. If a tuple is present, it must be
    // exactly the tuple this launcher would reserve, otherwise fail closed
    // instead of silently dropping a live checkout's ports.
    const expectedPorts = portsForOffset(entry.offset);
    if (
      entry.ports !== undefined &&
      (!Array.isArray(entry.ports) ||
        entry.ports.length !== expectedPorts.length ||
        entry.ports.some((port, portIndex) => port !== expectedPorts[portIndex]))
    ) {
      throw new Error(
        `development port reservation registry ${registryPath} contains an invalid port tuple at index ${index}; regenerate it only after confirming no other worktree is running`,
      );
    }
    if (!existsSync(entry.repoRoot) || !existsSync(entry.envFile)) return [];
    return [{ ...entry, ports: expectedPorts }];
  });
}

function writePortReservationRegistry(
  entries,
  registryPath = DEV_PORT_RESERVATION_REGISTRY_FILE,
) {
  mkdirSync(dirname(registryPath), { recursive: true, mode: 0o700 });
  const temporaryPath = `${registryPath}.${process.pid}.${randomUUID()}.tmp`;
  let operationError;
  try {
    writeFileSync(
      temporaryPath,
      `${JSON.stringify(
        {
          schema: DEV_PORT_RESERVATION_REGISTRY_SCHEMA,
          reservations: entries,
        },
        null,
        2,
      )}\n`,
      { encoding: "utf8", mode: 0o600 },
    );
    renameSync(temporaryPath, registryPath);
  } catch (error) {
    operationError = error;
  }
  let cleanupError;
  try {
    unlinkSync(temporaryPath);
  } catch (error) {
    if (error?.code !== "ENOENT") cleanupError = error;
  }
  if (operationError) throw operationError;
  if (cleanupError) throw cleanupError;
}

function registerPortReservation({
  repoRoot,
  envFile,
  offset,
  registryPath = DEV_PORT_RESERVATION_REGISTRY_FILE,
}) {
  const root = resolve(repoRoot);
  const file = resolve(envFile);
  const current = readPortReservationRegistry(registryPath).filter(
    (entry) => entry.repoRoot !== root,
  );
  current.push({
    checkoutId: checkoutIdentity(root),
    repoRoot: root,
    envFile: file,
    offset,
    ports: portsForOffset(offset),
    updatedAt: new Date().toISOString(),
  });
  writePortReservationRegistry(current, registryPath);
}

function unregisterPortReservation({
  repoRoot,
  registryPath = DEV_PORT_RESERVATION_REGISTRY_FILE,
}) {
  const root = resolve(repoRoot);
  const current = readPortReservationRegistry(registryPath).filter(
    (entry) => entry.repoRoot !== root,
  );
  if (current.length === 0) {
    try {
      unlinkSync(registryPath);
    } catch (error) {
      if (error?.code !== "ENOENT") throw error;
    }
    return;
  }
  writePortReservationRegistry(current, registryPath);
}

function reservedWorktreePorts(
  repoRoot,
  { registryPath = DEV_PORT_RESERVATION_REGISTRY_FILE } = {},
) {
  const reserved = new Set();
  const currentRoot = resolve(repoRoot);
  let output = "";
  try {
    output = execFileSync("git", ["worktree", "list", "--porcelain"], {
      cwd: repoRoot,
      encoding: "utf8",
      stdio: ["ignore", "pipe", "ignore"],
    });
  } catch {
    output = "";
  }
  for (const line of output.split("\n")) {
    if (!line.startsWith("worktree ")) continue;
    const checkout = line.slice("worktree ".length);
    for (const name of [".env.worktree", ".env"]) {
      const candidate = join(checkout, name);
      if (!existsSync(candidate)) continue;
      try {
        const values = parseEnv(readFileSync(candidate, "utf8"));
        for (const key of ["PORT", "FRONTEND_PORT", "DESKTOP_RENDERER_PORT"]) {
          if (/^[0-9]+$/.test(values[key] || ""))
            reserved.add(Number(values[key]));
        }
      } catch {
        // A malformed sibling environment should not prevent this checkout
        // from selecting ports that are demonstrably free.
      }
      break;
    }
  }
  for (const entry of readPortReservationRegistry(registryPath)) {
    // A force-regenerated env for this same checkout may keep its current
    // tuple. Other checkouts, including independent clones, must reserve it.
    if (entry.repoRoot === currentRoot) continue;
    for (const port of entry.ports || portsForOffset(entry.offset)) {
      reserved.add(port);
    }
  }
  return reserved;
}

async function portIsAvailable(port) {
  return new Promise((resolvePort, rejectPort) => {
    const server = createServer();
    server.unref();
    server.once("error", (error) => {
      if (error?.code === "EADDRINUSE") {
        resolvePort(false);
        return;
      }
      rejectPort(error);
    });
    server.listen({ host: "127.0.0.1", port, exclusive: true }, () => {
      server.close(() => resolvePort(true));
    });
  });
}

function waitForPortReservationLock() {
  return new Promise((resolveWait) => setTimeout(resolveWait, 50));
}

/**
 * Serialize the short "scan, probe, and write" critical section used when a
 * checkout first receives its development ports. The probe sockets close
 * before the env file is written, so without a user-level lock two worktrees
 * can both select the same apparently-free tuple. The lock is deliberately
 * outside either checkout so linked worktrees and independent clones of this
 * repository coordinate on the same host.
 */
export async function withDevPortReservationLock(
  operation,
  { lockPath = DEV_PORT_RESERVATION_LOCK_FILE } = {},
) {
  const token = `${process.pid}:${randomUUID()}\n`;
  const deadline = Date.now() + DEV_PORT_RESERVATION_LOCK_WAIT_MS;

  while (true) {
    try {
      writeFileSync(lockPath, token, {
        encoding: "utf8",
        flag: "wx",
        mode: 0o600,
      });
      break;
    } catch (error) {
      if (error?.code !== "EEXIST") throw error;
      try {
        const lockStat = statSync(lockPath);
        if (Date.now() - lockStat.mtimeMs > DEV_PORT_RESERVATION_LOCK_STALE_MS) {
          unlinkSync(lockPath);
          continue;
        }
      } catch (statError) {
        if (statError?.code !== "ENOENT") throw statError;
        continue;
      }
      if (Date.now() >= deadline) {
        throw new Error(
          `timed out waiting for the development port reservation lock: ${lockPath}`,
        );
      }
      await waitForPortReservationLock();
    }
  }

  let result;
  let operationError;
  try {
    result = await operation();
  } catch (error) {
    operationError = error;
  }

  let releaseError;
  try {
    if (readFileSync(lockPath, "utf8") === token) {
      unlinkSync(lockPath);
    }
  } catch (error) {
    if (error?.code !== "ENOENT") releaseError = error;
  }
  if (operationError) throw operationError;
  if (releaseError) throw releaseError;
  return result;
}

export async function allocateWorktreeOffset(
  repoRoot,
  {
    portCheck = portIsAvailable,
    reservationRegistryPath = DEV_PORT_RESERVATION_REGISTRY_FILE,
  } = {},
) {
  const reserved = reservedWorktreePorts(repoRoot, {
    registryPath: reservationRegistryPath,
  });
  const initial = offsetForPath(repoRoot);
  for (let attempt = 0; attempt < 1000; attempt += 1) {
    const offset = (initial + attempt) % 1000;
    const ports = portsForOffset(offset);
    if (ports.some((port) => reserved.has(port))) continue;
    let available;
    try {
      available = (await Promise.all(ports.map(portCheck))).every(Boolean);
    } catch (error) {
      const code = error?.code;
      if (code === "EACCES" || code === "EPERM") {
        throw new Error(
          "could not check isolated development ports: the operating system denied binding to 127.0.0.1; run the launcher outside a restricted sandbox or grant local bind permission",
        );
      }
      throw new Error(
        `could not check isolated development ports: ${error instanceof Error ? error.message : String(error)}`,
      );
    }
    if (available) {
      return offset;
    }
  }
  throw new Error(
    "could not allocate isolated backend, frontend, and Electron renderer ports; stop stale development processes or remove obsolete worktree env files",
  );
}

function worktreeEnvContents(
  repoRoot,
  offset,
  worktreeName = basename(repoRoot),
) {
  const slug = checkoutSlug(worktreeName);
  const checkoutId = checkoutIdentity(repoRoot);
  const postgresDb = `patchbay_${slug}_${checkoutId.slice(0, 8)}_${offset}`;
  const backendPort = 18080 + offset;
  const frontendPort = 13000 + offset;
  const frontendOrigin = `http://localhost:${frontendPort}`;
  return `POSTGRES_DB=${postgresDb}
POSTGRES_USER=patchbay
POSTGRES_PASSWORD=patchbay
POSTGRES_PORT=5432
DATABASE_URL=postgres://patchbay:patchbay@localhost:5432/${postgresDb}?sslmode=disable

PATCHBAY_DEV_ENV_SCHEMA=${DEV_CHECKOUT_ENV_SCHEMA}
PATCHBAY_DEV_CHECKOUT_ID=${checkoutId}
PATCHBAY_DEV_CHECKOUT_SLUG=${slug}
PATCHBAY_DEV_CHECKOUT_OFFSET=${offset}

APP_ENV=development
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
DESKTOP_RENDERER_PORT=${rendererPortForOffset(offset)}
DESKTOP_APP_SUFFIX=${appSuffixForOffset(repoRoot, offset)}
`;
}

export function validateGeneratedDevCheckoutEnv({ repoRoot, values }) {
  if (values.PATCHBAY_DEV_ENV_SCHEMA !== DEV_CHECKOUT_ENV_SCHEMA) {
    return `schema ${values.PATCHBAY_DEV_ENV_SCHEMA || "missing"} is not ${DEV_CHECKOUT_ENV_SCHEMA}`;
  }
  if (values.PATCHBAY_DEV_CHECKOUT_ID !== checkoutIdentity(repoRoot)) {
    return "checkout identity does not match this path";
  }
  const offset = Number(values.PATCHBAY_DEV_CHECKOUT_OFFSET);
  if (!Number.isInteger(offset) || offset < 0 || offset >= 1000) {
    return "checkout offset is invalid";
  }
  const slug = values.PATCHBAY_DEV_CHECKOUT_SLUG;
  if (
    !slug ||
    slug !== checkoutSlug(slug) ||
    !/^[a-z0-9]+(?:_[a-z0-9]+)*$/.test(slug)
  ) {
    return "checkout slug is invalid";
  }
  const expectedDatabase = `patchbay_${slug}_${checkoutIdentity(repoRoot).slice(0, 8)}_${offset}`;
  if (values.POSTGRES_DB !== expectedDatabase) {
    return `database ${values.POSTGRES_DB || "missing"} does not match ${expectedDatabase}`;
  }
  const databaseUrl = values.DATABASE_URL || "";
  if (!new RegExp(`/${expectedDatabase}(?:\\?|$)`).test(databaseUrl)) {
    return "DATABASE_URL does not point at the isolated database";
  }
  if (Number(values.PORT) !== 18080 + offset) {
    return "backend port does not match the isolated offset";
  }
  if (Number(values.FRONTEND_PORT) !== 13000 + offset) {
    return "frontend port does not match the isolated offset";
  }
  if (Number(values.DESKTOP_RENDERER_PORT) !== rendererPortForOffset(offset)) {
    return "Electron renderer port does not match the isolated offset";
  }
  if (values.DESKTOP_APP_SUFFIX !== appSuffixForOffset(repoRoot, offset)) {
    return "Electron app identity does not match the isolated checkout";
  }
  return null;
}

export async function createWorktreeEnvFile({
  repoRoot,
  envFile = join(repoRoot, ".env.worktree"),
  force = false,
  worktreeName,
  allocateOffset = allocateWorktreeOffset,
  reservationLockPath = DEV_PORT_RESERVATION_LOCK_FILE,
  reservationRegistryPath = DEV_PORT_RESERVATION_REGISTRY_FILE,
} = {}) {
  return withDevPortReservationLock(
    async () => {
      if (existsSync(envFile) && !force) {
        throw new Error(
          `Refusing to overwrite existing ${envFile}. Re-run with FORCE=1 if you want to regenerate it.`,
        );
      }
      const offset = await allocateOffset(repoRoot, {
        reservationRegistryPath,
      });
      registerPortReservation({
        repoRoot,
        envFile,
        offset,
        registryPath: reservationRegistryPath,
      });
      try {
        writeFileSync(
          envFile,
          worktreeEnvContents(repoRoot, offset, worktreeName),
          { mode: 0o600 },
        );
      } catch (error) {
        unregisterPortReservation({
          repoRoot,
          registryPath: reservationRegistryPath,
        });
        throw error;
      }
      return { envFile, offset };
    },
    { lockPath: reservationLockPath },
  );
}

export async function ensureDevCheckoutEnv({
  repoRoot,
  env = process.env,
  log = console,
  allocateOffset = allocateWorktreeOffset,
  reservationLockPath = DEV_PORT_RESERVATION_LOCK_FILE,
  reservationRegistryPath = DEV_PORT_RESERVATION_REGISTRY_FILE,
} = {}) {
  const envFile = selectDevEnvFile({ repoRoot, env });
  if (!existsSync(envFile)) {
    await createWorktreeEnvFile({
      repoRoot,
      envFile,
      allocateOffset,
      reservationLockPath,
      reservationRegistryPath,
    });
    log.log(`[dev] generated isolated checkout environment ${envFile}`);
  } else if (!env.PATCHBAY_DEV_ENV_FILE) {
    let issue;
    let values;
    try {
      values = parseEnv(readFileSync(envFile, "utf8"));
      issue = validateGeneratedDevCheckoutEnv({
        repoRoot,
        values,
      });
    } catch (error) {
      issue = error instanceof Error ? error.message : String(error);
    }
    if (issue) {
      throw new Error(
        `development env file ${envFile} is not a current isolated checkout environment: ${issue}. Re-run FORCE=1 make worktree-env and then pnpm dev.`,
      );
    }
    await withDevPortReservationLock(
      async () =>
        registerPortReservation({
          repoRoot,
          envFile,
          offset: Number(values.PATCHBAY_DEV_CHECKOUT_OFFSET),
          registryPath: reservationRegistryPath,
        }),
      { lockPath: reservationLockPath },
    );
  }
  const secrets = await ensureSecretsFile(envFile);
  if (secrets.generated.length > 0) {
    log.log(
      `[dev] generated local-only integration encryption keys in ${envFile}: ${secrets.generated.join(", ")}`,
    );
  }
  return loadDevCheckoutEnv({ repoRoot, env, envFile });
}
