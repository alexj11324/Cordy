#!/usr/bin/env node

import {
  chmod,
  readFile,
  realpath,
  rename,
  stat,
  unlink,
  writeFile,
} from "node:fs/promises";
import { randomUUID } from "node:crypto";
import path from "node:path";
import { fileURLToPath } from "node:url";

const LEGACY_PROPERTY = "ios.buildReactNativeFromSource";
const defaultPropertiesPath = fileURLToPath(
  new URL("../ios/Podfile.properties.json", import.meta.url),
);

export async function disableIosSourceBuild(propertiesPath = defaultPropertiesPath) {
  let source;
  let sourceStat;

  try {
    [source, sourceStat] = await Promise.all([
      readFile(propertiesPath, "utf8"),
      stat(propertiesPath),
    ]);
  } catch (error) {
    if (error?.code === "ENOENT") {
      return false;
    }
    throw error;
  }

  const properties = JSON.parse(source);
  const legacyValue = properties[LEGACY_PROPERTY];
  if (legacyValue !== true && legacyValue !== "true") {
    return false;
  }

  delete properties[LEGACY_PROPERTY];

  const temporaryPath = path.join(
    path.dirname(propertiesPath),
    `.${path.basename(propertiesPath)}.${process.pid}.${randomUUID()}.tmp`,
  );

  try {
    await writeFile(temporaryPath, `${JSON.stringify(properties, null, 2)}\n`, {
      flag: "wx",
      mode: sourceStat.mode,
    });
    await chmod(temporaryPath, sourceStat.mode);
    await rename(temporaryPath, propertiesPath);
  } catch (error) {
    await unlink(temporaryPath).catch(() => {});
    throw error;
  }

  return true;
}

const isMain =
  process.argv[1] &&
  (await realpath(process.argv[1])) === (await realpath(fileURLToPath(import.meta.url)));

if (isMain) {
  await disableIosSourceBuild(process.argv[2]);
}
