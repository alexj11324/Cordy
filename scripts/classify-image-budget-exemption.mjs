import { readFile } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";

const EXEMPTION_PATTERN = /^\s*Oversized image exemption:\s*\S/im;

export function hasImageBudgetExemption(bodies) {
  return bodies.some((body) => EXEMPTION_PATTERN.test(body ?? ""));
}

function bodyFiles(argv) {
  const files = [];
  for (let index = 0; index < argv.length; index += 1) {
    if (argv[index] !== "--body-file") continue;
    const file = argv[++index];
    if (!file) throw new Error("--body-file requires a path");
    files.push(file);
  }
  return files;
}

async function main() {
  const files = bodyFiles(process.argv.slice(2));
  const bodies = await Promise.all(files.map((file) => readFile(file, "utf8")));
  console.log(`image_budget_exempt=${String(hasImageBudgetExemption(bodies))}`);
}

const entrypoint = process.argv[1] && path.resolve(process.argv[1]);
if (!process.execArgv.includes("--test") && entrypoint === fileURLToPath(import.meta.url)) {
  await main();
}
