#!/usr/bin/env node
// Prepare the Windows App delta for a release from the previous published
// package. Publication downloads that package, then this script adds the delta
// artifact and descriptor record before the signed index is generated.

import {
  copyFileSync,
  existsSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { extractZip } from "./archive-tool.mjs";
import { createAppDelta } from "./package-app-delta.mjs";

function fail(message) {
  throw new Error(message);
}

const [artifactsDirectory, previousArchive, previousVersion] = process.argv.slice(2);
if (!artifactsDirectory || !previousArchive || !previousVersion) {
  fail("Usage: node scripts/prepare-app-deltas.mjs <artifacts-directory> <previous-windows-zip> <previous-version>");
}
const root = resolve(artifactsDirectory);
const archive = join(root, "piik-app-windows-amd64.zip");
const descriptorPath = join(root, "piik-app-windows-amd64.release.json");
if (!existsSync(archive) || !existsSync(descriptorPath)) fail("Windows App artifacts are missing");
const descriptor = JSON.parse(readFileSync(descriptorPath, "utf8"));
if (descriptor.target !== "windows-amd64") fail("Windows App descriptor is invalid");

const staging = mkdtempSync(join(tmpdir(), "piik-app-deltas-"));
try {
  const current = join(staging, "piik-app-windows-amd64");
  mkdirSync(current);
  extractZip(archive, current);
  const delta = createAppDelta({
    currentDirectory: current,
    previousArchive: resolve(previousArchive),
    fromVersion: previousVersion,
    target: "windows-amd64",
    outputDirectory: join(staging, "delta"),
  });
  copyFileSync(join(staging, "delta", delta.artifact), join(root, delta.artifact));
  copyFileSync(join(staging, "delta", `${delta.artifact}.json`), join(root, `${delta.artifact}.json`));
  descriptor.deltas = [...(descriptor.deltas ?? []).filter(existing => existing.from !== previousVersion), delta];
  writeFileSync(descriptorPath, `${JSON.stringify(descriptor, null, 2)}\n`);
  process.stdout.write(`${JSON.stringify({
    from: previousVersion, artifact: delta.artifact, size: delta.size, files: delta.files.length,
  })}\n`);
} finally {
  rmSync(staging, { recursive: true, force: true });
}
