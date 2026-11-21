#!/usr/bin/env node
// Build one delta package from the files that changed between two verified App
// packages. The signed update index carries the changed-file records; this
// module produces the artifact and its publisher descriptor.

import { createHash } from "node:crypto";
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
import { basename, dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { createZip, extractZip } from "./archive-tool.mjs";
import { fileRecords } from "./package-files.mjs";

function fail(message) {
  throw new Error(message);
}

function sha256(path) {
  return createHash("sha256").update(readFileSync(path)).digest("hex");
}

/**
 * Compare a complete package directory with one previous package archive and
 * write the changed-files delta plus its descriptor into a new directory.
 */
export function createAppDelta({ currentDirectory, previousArchive, fromVersion, target, outputDirectory }) {
  if (!/^v(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)$/.test(fromVersion ?? "")) {
    fail("from-version must be a stable vMAJOR.MINOR.PATCH");
  }
  const currentRoot = resolve(currentDirectory);
  if (!existsSync(join(currentRoot, "piik-app.exe"))) fail("Current package directory is incomplete");
  const output = resolve(outputDirectory);
  if (existsSync(output)) fail("Delta output directory must not exist");

  const staging = mkdtempSync(join(tmpdir(), "piik-delta-"));
  try {
    const previousRoot = join(staging, "previous");
    mkdirSync(previousRoot);
    extractZip(resolve(previousArchive), previousRoot);
    const previous = new Map(fileRecords(previousRoot).map(file => [file.path, file]));
    const changed = fileRecords(currentRoot).filter(file => {
      const before = previous.get(file.path);
      return !before || before.size !== file.size || before.sha256 !== file.sha256;
    });
    if (changed.length === 0) fail("No changed files between these packages");
    const deltaRoot = join(staging, "delta");
    for (const file of changed) {
      const destination = join(deltaRoot, file.path);
      mkdirSync(dirname(destination), { recursive: true });
      copyFileSync(join(currentRoot, file.path), destination);
    }
    mkdirSync(output, { recursive: false });
    const artifact = `piik-app-${target}-delta-${fromVersion}.zip`;
    const archive = join(output, artifact);
    createZip(deltaRoot, archive);
    const descriptor = {
      from: fromVersion,
      artifact,
      size: readFileSync(archive).length,
      sha256: sha256(archive),
      files: changed,
    };
    writeFileSync(join(output, `${artifact}.json`), `${JSON.stringify(descriptor, null, 2)}\n`);
    return descriptor;
  } finally {
    rmSync(staging, { recursive: true, force: true });
  }
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  try {
    const [currentDirectory, previousArchive, fromVersion, outputDirectory] = process.argv.slice(2);
    if (!currentDirectory || !previousArchive || !fromVersion || !outputDirectory) {
      fail("Usage: node scripts/package-app-delta.mjs <current-package-directory> <previous-package.zip> <from-version> <new-output-directory>");
    }
    const directoryName = basename(resolve(currentDirectory));
    if (!directoryName.startsWith("piik-app-") || directoryName.length === "piik-app-".length) {
      fail("Current package directory must be named piik-app-<target>");
    }
    const descriptor = createAppDelta({
      currentDirectory,
      previousArchive,
      fromVersion,
      target: directoryName.slice("piik-app-".length),
      outputDirectory,
    });
    process.stdout.write(`${JSON.stringify({ ...descriptor, files: descriptor.files.length })}\n`);
  } catch (error) {
    process.stderr.write(`${error.message}\n`);
    process.exitCode = 1;
  }
}
