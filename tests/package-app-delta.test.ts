import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import { mkdirSync, mkdtempSync, readFileSync, readdirSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";
import { createZip, extractZip } from "../scripts/archive-tool.mjs";

const script = fileURLToPath(new URL("../scripts/package-app-delta.mjs", import.meta.url));
const sha = (path: string) => createHash("sha256").update(readFileSync(path)).digest("hex");

describe("App delta package", () => {
  it("carries only changed files and describes them for the signed index", () => {
    const root = mkdtempSync(join(tmpdir(), "piik-app-delta-"));
    try {
      const previous = join(root, "previous");
      const current = join(root, "piik-app-windows-amd64");
      const files = [
        ["piik-app.exe", "old app", "new app"],
        ["web/index.html", "old web", "new web"],
        ["runtime/tunnel/cloudflared.exe", "same tunnel", "same tunnel"],
        ["REVISION", "old", "new"],
      ];
      for (const [path, oldValue, newValue] of files) {
        mkdirSync(dirname(join(previous, path)), { recursive: true });
        mkdirSync(dirname(join(current, path)), { recursive: true });
        writeFileSync(join(previous, path), oldValue);
        writeFileSync(join(current, path), newValue);
      }
      const previousArchive = join(root, "previous.zip");
      createZip(previous, previousArchive);
      const output = join(root, "delta");
      const result = JSON.parse(execFileSync(
        process.execPath,
        [script, current, previousArchive, "v2.9.0", output],
        { encoding: "utf8" },
      ));
      expect(result.from).toBe("v2.9.0");
      expect(result.artifact).toBe("piik-app-windows-amd64-delta-v2.9.0.zip");
      expect(result.files).toBe(3);
      expect(result.sha256).toBe(sha(join(output, result.artifact)));
      const descriptor = JSON.parse(readFileSync(join(output, `${result.artifact}.json`), "utf8"));
      expect(descriptor.files.map((file: { path: string }) => file.path))
        .toEqual(["REVISION", "piik-app.exe", "web/index.html"]);
      const extracted = join(root, "extracted");
      mkdirSync(extracted);
      extractZip(join(output, result.artifact), extracted);
      expect(readFileSync(join(extracted, "piik-app.exe"), "utf8")).toBe("new app");
      expect(readdirSync(extracted).sort()).toEqual(["REVISION", "piik-app.exe", "web"]);
    } finally {
      rmSync(root, { recursive: true, force: true });
    }
  });
});
