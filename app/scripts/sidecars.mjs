// Downloads the pinned ffmpeg and QuickJS builds for one Rust target triple, checks their
// SHA-256 and puts them where Tauri's `externalBin` expects them:
// src-tauri/binaries/<name>-<triple>[.exe]
//
// Usage: node scripts/sidecars.mjs [triple]   (default: the host triple from `rustc -vV`)

import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import { chmodSync, copyFileSync, existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const app = join(dirname(fileURLToPath(import.meta.url)), "..");
const pins = JSON.parse(readFileSync(join(app, "sidecars.json"), "utf8"));
const triple =
  process.argv[2] ?? /host: (\S+)/.exec(execFileSync("rustc", ["-vV"], { encoding: "utf8" }))[1];
const tools = pins[triple];
if (!tools) {
  console.error(`no sidecars pinned for ${triple}; known: ${Object.keys(pins).join(", ")}`);
  process.exit(1);
}

const cache = join(app, ".sidecar-cache");
const out = join(app, "src-tauri", "binaries");
mkdirSync(cache, { recursive: true });
mkdirSync(out, { recursive: true });
const exe = triple.includes("windows") ? ".exe" : "";
const sha256 = (buf) => createHash("sha256").update(buf).digest("hex");

for (const [name, pin] of Object.entries(tools)) {
  const dest = join(out, `${name}-${triple}${exe}`);
  const stamp = `${dest}.sha256`;
  if (existsSync(dest) && existsSync(stamp) && readFileSync(stamp, "utf8") === pin.sha256) {
    console.log(`✓ ${name} (${triple}) up to date`);
    continue;
  }

  const file = join(cache, pin.sha256);
  if (!existsSync(file) || sha256(readFileSync(file)) !== pin.sha256) {
    console.log(`↓ ${pin.url}`);
    const res = await fetch(pin.url);
    if (!res.ok) throw new Error(`${pin.url}: HTTP ${res.status}`);
    const body = Buffer.from(await res.arrayBuffer());
    const got = sha256(body);
    if (got !== pin.sha256) throw new Error(`${pin.url}: sha256 ${got}, expected ${pin.sha256}`);
    writeFileSync(file, body);
  }

  if (pin.member) {
    // bsdtar reads zip archives: macOS's tar, and Windows' own (not Git Bash's GNU tar).
    const tar =
      process.platform === "win32" ? join(process.env.SystemRoot ?? "C:\\Windows", "System32", "tar.exe") : "tar";
    const work = mkdtempSync(join(tmpdir(), "sidecar-"));
    execFileSync(tar, ["-xf", file, "-C", work, pin.member]);
    copyFileSync(join(work, pin.member), dest);
    rmSync(work, { recursive: true, force: true });
  } else {
    copyFileSync(file, dest);
  }
  chmodSync(dest, 0o755);
  writeFileSync(stamp, pin.sha256);
  console.log(`✓ ${name} → ${dest}`);
}
