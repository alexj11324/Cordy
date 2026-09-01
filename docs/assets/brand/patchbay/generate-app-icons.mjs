#!/usr/bin/env node

import { spawnSync } from "node:child_process";
import {
  mkdirSync,
  mkdtempSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const root = dirname(fileURLToPath(import.meta.url));
const mark = join(root, "mark-on-dark.png");
const output = join(root, "app-icons");
const source = join(output, "app-icon-1024.png");
const temporary = mkdtempSync(join(tmpdir(), "patchbay-app-icons-"));

const linuxSizes = [16, 24, 32, 48, 64, 128, 256, 512];
const windowsSizes = [16, 24, 32, 48, 64, 128, 256];
const macSizes = [16, 32, 64, 128, 256, 512, 1024];

function run(command, args) {
  const result = spawnSync(command, args, { stdio: "inherit" });
  if (result.error) throw result.error;
  if (result.status !== 0) {
    throw new Error(`${command} exited with status ${result.status}`);
  }
}

function renderPng(input, size, destination) {
  run("ffmpeg", [
    "-v",
    "error",
    "-i",
    input,
    "-vf",
    `scale=${size}:${size}:flags=lanczos`,
    "-frames:v",
    "1",
    "-map_metadata",
    "-1",
    "-y",
    destination,
  ]);
}

function makeIco(images, destination) {
  const headerSize = 6 + images.length * 16;
  const header = Buffer.alloc(headerSize);
  header.writeUInt16LE(0, 0);
  header.writeUInt16LE(1, 2);
  header.writeUInt16LE(images.length, 4);

  let offset = headerSize;
  images.forEach(({ size, data }, index) => {
    const entry = 6 + index * 16;
    header.writeUInt8(size === 256 ? 0 : size, entry);
    header.writeUInt8(size === 256 ? 0 : size, entry + 1);
    header.writeUInt8(0, entry + 2);
    header.writeUInt8(0, entry + 3);
    header.writeUInt16LE(1, entry + 4);
    header.writeUInt16LE(32, entry + 6);
    header.writeUInt32LE(data.length, entry + 8);
    header.writeUInt32LE(offset, entry + 12);
    offset += data.length;
  });

  writeFileSync(destination, Buffer.concat([header, ...images.map(({ data }) => data)]));
}

function makeIcns(images, destination) {
  const imagesBySize = new Map(images.map(({ size, data }) => [size, data]));
  const representations = [
    ["icp4", 16],
    ["icp5", 32],
    ["icp6", 64],
    ["ic07", 128],
    ["ic08", 256],
    ["ic09", 512],
    ["ic10", 1024],
    ["ic11", 32],
    ["ic12", 64],
    ["ic13", 256],
    ["ic14", 512],
  ];
  const chunks = representations.map(([type, size]) => {
    const data = imagesBySize.get(size);
    const chunk = Buffer.alloc(8 + data.length);
    chunk.write(type, 0, 4, "ascii");
    chunk.writeUInt32BE(chunk.length, 4);
    data.copy(chunk, 8);
    return chunk;
  });
  const header = Buffer.alloc(8);
  header.write("icns", 0, 4, "ascii");
  header.writeUInt32BE(8 + chunks.reduce((sum, chunk) => sum + chunk.length, 0), 4);
  writeFileSync(destination, Buffer.concat([header, ...chunks]));
}

mkdirSync(join(output, "linux"), { recursive: true });
mkdirSync(join(output, "macos"), { recursive: true });
mkdirSync(join(output, "windows"), { recursive: true });

try {
  // The transparent brand mark remains unchanged. The dark squircle gives the
  // light sockets reliable contrast on every desktop background.
  run("ffmpeg", [
    "-v",
    "error",
    "-f",
    "lavfi",
    "-i",
    "color=c=0x111111:s=1024x1024,format=rgba",
    "-i",
    mark,
    "-filter_complex",
    "[0:v]geq=r='17':g='17':b='17':a='if(lte(pow(abs((X-W/2)/(W/2)),4)+pow(abs((Y-H/2)/(H/2)),4),1),255,0)'[bg];[bg][1:v]overlay=format=auto",
    "-frames:v",
    "1",
    "-map_metadata",
    "-1",
    "-y",
    source,
  ]);

  const rendered = new Map([[1024, source]]);
  for (const size of new Set([...linuxSizes, ...windowsSizes, ...macSizes])) {
    if (size === 1024) continue;
    const path = join(temporary, `${size}x${size}.png`);
    renderPng(source, size, path);
    rendered.set(size, path);
  }

  for (const size of linuxSizes) {
    writeFileSync(
      join(output, "linux", `${size}x${size}.png`),
      readFileSync(rendered.get(size)),
    );
  }

  makeIco(
    windowsSizes.map((size) => ({ size, data: readFileSync(rendered.get(size)) })),
    join(output, "windows", "Patchbay.ico"),
  );
  makeIcns(
    macSizes.map((size) => ({ size, data: readFileSync(rendered.get(size)) })),
    join(output, "macos", "Patchbay.icns"),
  );
} finally {
  rmSync(temporary, { recursive: true, force: true });
}

console.log(`Generated Patchbay desktop icons in ${output}`);
