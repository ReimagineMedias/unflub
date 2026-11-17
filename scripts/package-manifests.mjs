#!/usr/bin/env node
// Writes the winget, Scoop and Chocolatey files for one published release, with its version and
// the SHA-256 of each setup taken from the release's SHA256SUMS.txt.
//
//   node scripts/package-manifests.mjs --version 0.2.0 --sums SHA256SUMS.txt --out <dir>
//
// The templates in packaging/ are at 0.1.0 with all-zero hashes. Every "0.1.0" becomes the
// version, and every all-zero hash becomes the hash of the setup named in the lines just above
// it (the installer URL). The run fails if a setup is missing from the sums or a placeholder
// hash is left, so nothing with a zero hash is ever submitted. Writes:
//
//   <dir>/winget/<version>/RMGN.unflub*.yaml, <dir>/scoop/unflub.json, <dir>/chocolatey/...

import { mkdirSync, readdirSync, readFileSync, statSync, writeFileSync } from "node:fs";
import { dirname, join, relative } from "node:path";
import { fileURLToPath } from "node:url";

function arg(name) {
  const i = process.argv.indexOf(`--${name}`);
  if (i > 0 && process.argv[i + 1]) return process.argv[i + 1];
  console.error(`missing --${name}`);
  process.exit(2);
}

const version = arg("version");
const sumsFile = arg("sums");
const out = arg("out");
if (!/^\d+\.\d+\.\d+$/.test(version)) throw new Error(`${version} is not a stable x.y.z version`);

const TEMPLATE = "0.1.0";
const ZERO = "0".repeat(64);
const root = join(dirname(fileURLToPath(import.meta.url)), "..", "packaging");

/** `file -> sha256` from GNU-style `sha256sum` output. */
const sums = new Map();
for (const line of readFileSync(sumsFile, "utf8").split(/\r?\n/)) {
  const m = /^([0-9a-f]{64}) [ *]?(.+)$/.exec(line.trim());
  if (m) sums.set(m[2], m[1]);
}

function render(text, where) {
  const lines = text.replaceAll(TEMPLATE, version).split("\n");
  for (let i = 0; i < lines.length; i++) {
    if (!lines[i].includes(ZERO)) continue;
    // The setup this hash belongs to is named in one of the few lines before it.
    let file = null;
    for (let j = i; j >= Math.max(0, i - 4) && !file; j--) {
      const m = /(unflub_[0-9.]+_(?:x64|arm64)-setup\.exe)/.exec(lines[j]);
      if (m) file = m[1];
    }
    if (!file) throw new Error(`${where}:${i + 1}: placeholder hash with no setup named above it`);
    const hash = sums.get(file);
    if (!hash) throw new Error(`${where}:${i + 1}: ${file} is not in ${sumsFile}`);
    lines[i] = lines[i].replace(ZERO, hash);
  }
  const done = lines.join("\n");
  if (done.includes(ZERO)) throw new Error(`${where}: a placeholder hash is left`);
  return done;
}

function* files(dir) {
  for (const name of readdirSync(dir)) {
    const path = join(dir, name);
    if (statSync(path).isDirectory()) yield* files(path);
    else yield path;
  }
}

for (const path of files(root)) {
  const rel = relative(root, path).replaceAll("\\", "/").replace(`winget/${TEMPLATE}/`, `winget/${version}/`);
  const target = join(out, rel);
  mkdirSync(dirname(target), { recursive: true });
  writeFileSync(target, render(readFileSync(path, "utf8"), rel));
  console.log(rel);
}
