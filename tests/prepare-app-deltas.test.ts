import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";
import { createZip } from "../scripts/archive-tool.mjs";
import { fileRecords } from "../scripts/package-files.mjs";

const script = fileURLToPath(new URL("../scripts/prepare-app-deltas.mjs", import.meta.url));
const revision = "a".repeat(40);
const sha = (path: string) => createHash("sha256").update(readFileSync(path)).digest("hex");

function writePackage(root: string, values: Record<string, string>) {
  for (const [path, value] of Object.entries(values)) {
    mkdirSync(dirname(join(root, path)), { recursive: true });
    writeFileSync(join(root, path), value);
  }
}

describe("release App delta preparation", () => {
  it("adds the previous-release delta to the Windows descriptor and artifacts", () => {
    const root = mkdtempSync(join(tmpdir(), "piik-prepare-deltas-"));
    try {
      const current = join(root, "package", "piik-app-windows-amd64");
      const previous = join(root, "previous");
      writePackage(current, {
        "piik-app.exe": "new app",
        "web/index.html": "new web",
        "runtime/tunnel/cloudflared.exe": "same tunnel",
        "REVISION": "new",
      });
      writePackage(previous, {
        "piik-app.exe": "old app",
        "web/index.html": "old web",
        "runtime/tunnel/cloudflared.exe": "same tunnel",
        "REVISION": "old",
      });
      const artifacts = join(root, "artifacts");
      mkdirSync(artifacts);
      const archive = join(artifacts, "piik-app-windows-amd64.zip");
      createZip(current, archive);
      writeFileSync(join(artifacts, "piik-app-windows-amd64.release.json"), JSON.stringify({
        schema: 2, version: "v3.0.2", revision, target: "windows-amd64",
        artifact: "piik-app-windows-amd64.zip", artifactSha256: sha(archive), files: fileRecords(current),
      }));
      const previousArchive = join(root, "piik-app-windows-amd64.zip");
      createZip(previous, previousArchive);
      const result = JSON.parse(execFileSync(process.execPath,
        [script, artifacts, previousArchive, "v3.0.1"], { encoding: "utf8" }));
      expect(result).toMatchObject({ from: "v3.0.1", files: 3 });
      const descriptor = JSON.parse(readFileSync(join(artifacts, "piik-app-windows-amd64.release.json"), "utf8"));
      expect(descriptor.deltas).toHaveLength(1);
      const delta = descriptor.deltas[0];
      expect(delta.artifact).toBe("piik-app-windows-amd64-delta-v3.0.1.zip");
      expect(delta.files.map((file: { path: string }) => file.path))
        .toEqual(["REVISION", "piik-app.exe", "web/index.html"]);
      expect(delta.sha256).toBe(sha(join(artifacts, delta.artifact)));
    } finally {
      rmSync(root, { recursive: true, force: true });
    }
  });
});
