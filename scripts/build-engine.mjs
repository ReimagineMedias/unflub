#!/usr/bin/env node
// Builds unflub-engine.exe and puts it where Tauri bundles it from (bundle > externalBin in
// app/src-tauri/tauri.conf.json): app/src-tauri/binaries/unflub-engine-<target triple>.exe.
// The installer then holds it beside unflub.exe as unflub-engine.exe.
//
//   node scripts/build-engine.mjs            release build for this machine
//   node scripts/build-engine.mjs --debug    debug build
//
// `npm run tauri build` runs it from beforeBuildCommand. The Tauri CLI sets
// TAURI_ENV_TARGET_TRIPLE (from --target, or the host) and TAURI_ENV_DEBUG, which win over the
// defaults above. CARGO_TARGET_DIR is honoured: the path comes from Cargo's own output.

import { execFileSync, spawnSync } from "node:child_process";
import { copyFileSync, mkdirSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");

const host = execFileSync("rustc", ["-vV"], { encoding: "utf8" }).match(/^host: (\S+)$/m)[1];
const triple = process.env.TAURI_ENV_TARGET_TRIPLE || host;
const debug = process.env.TAURI_ENV_DEBUG
  ? process.env.TAURI_ENV_DEBUG === "true"
  : process.argv.includes("--debug");

const args = ["build", "-p", "unflub-engine", "--bin", "unflub-engine", "--locked", "--message-format=json-render-diagnostics"];
if (!debug) args.push("--release");
// Only name the target when cross-building, so a plain build shares target/release with the app.
if (triple !== host) args.push("--target", triple);

console.error(`build-engine: cargo ${args.join(" ")}`);
const cargo = spawnSync("cargo", args, { cwd: root, encoding: "utf8", stdio: ["ignore", "pipe", "inherit"], maxBuffer: 64 * 1024 * 1024 });
if (cargo.status !== 0) {
  console.error(`build-engine: cargo failed (${cargo.status ?? cargo.error})`);
  process.exit(1);
}

let exe = null;
for (const line of cargo.stdout.split("\n")) {
  if (!line.startsWith("{")) continue;
  const msg = JSON.parse(line);
  if (msg.reason === "compiler-artifact" && msg.target?.name === "unflub-engine" && msg.executable) {
    exe = msg.executable;
  }
}
if (!exe) {
  console.error("build-engine: cargo reported no unflub-engine executable");
  process.exit(1);
}

const ext = triple.includes("windows") ? ".exe" : "";
const dest = join(root, "app", "src-tauri", "binaries", `unflub-engine-${triple}${ext}`);
mkdirSync(dirname(dest), { recursive: true });
copyFileSync(exe, dest);
console.error(`build-engine: ${exe} -> ${dest}`);
