#!/usr/bin/env node

import { createConnection } from "node:net";
import { pathToFileURL } from "node:url";

const RELEASE_TIMEOUT_MS = 10_000;

export function parseReleaseEndpoint(env = process.env) {
  const portValue = env.PATCHBAY_DEV_ACCEPTANCE_RELEASE_PORT;
  const token = env.PATCHBAY_DEV_ACCEPTANCE_RELEASE_TOKEN;
  if (portValue === undefined && token === undefined) return null;
  const port = Number(portValue);
  if (
    !Number.isInteger(port) ||
    port < 1024 ||
    port > 65535 ||
    typeof token !== "string" ||
    !/^[0-9a-f-]{36}$/i.test(token)
  ) {
    throw new Error(
      "invalid dev acceptance release endpoint; rerun the acceptance command so its reservation can be handed off safely",
    );
  }
  return { port, token };
}

export async function releaseDevAcceptancePort({
  endpoint = parseReleaseEndpoint(),
  connect = createConnection,
  timeoutMs = RELEASE_TIMEOUT_MS,
} = {}) {
  if (!endpoint) return false;
  await new Promise((resolve, reject) => {
    const socket = connect({ host: "127.0.0.1", port: endpoint.port });
    let response = "";
    let settled = false;
    let timeout;
    const finish = (error) => {
      if (settled) return;
      settled = true;
      clearTimeout(timeout);
      if (error) reject(error);
      else resolve();
    };
    timeout = setTimeout(() => {
      socket.destroy();
      finish(new Error("timed out waiting for the acceptance CDP reservation handoff"));
    }, timeoutMs);
    socket.setEncoding("utf8");
    socket.once("error", (error) => {
      finish(new Error(`could not contact the acceptance CDP reservation: ${error.message}`));
    });
    socket.on("data", (chunk) => {
      response += chunk;
      if (!response.includes("\n")) return;
      socket.end();
      if (response.trim() === "released") {
        finish();
      } else {
        finish(new Error("acceptance CDP reservation refused the handoff"));
      }
    });
    socket.on("connect", () => socket.write(`${endpoint.token}\n`));
  });
  return true;
}

if (
  process.argv[1] &&
  import.meta.url === pathToFileURL(process.argv[1]).href
) {
  releaseDevAcceptancePort().catch((error) => {
    console.error(`✗ ${error.message}`);
    process.exitCode = 1;
  });
}
