#!/usr/bin/env node
// Writes the updater's latest.json from the signed bundles in one folder.
//
//   node scripts/updater-manifest.mjs --dir <assets> --version 0.2.0 --tag v0.2.0 [--repo owner/name] [--notes text]
//
// The release workflow builds each platform in parallel and runs this once afterwards, so no two
// jobs race to rewrite latest.json on the release. Every `<bundle>.sig` becomes an entry:
//
//   windows-<arch>-nsis / -msi, linux-<arch>-appimage / -deb / -rpm, darwin-<arch>-app
//
// plus the plain `<os>-<arch>` key the updater falls back to (NSIS on Windows, AppImage on Linux).
// The updater looks for `<os>-<arch>-<installer>` first, so an MSI install keeps updating from the
// MSI and a setup.exe install from the setup.exe. An unrecognised .sig fails the run.

import { readdirSync, readFileSync } from "node:fs";
import { join } from "node:path";

function arg(name, fallback) {
  const i = process.argv.indexOf(`--${name}`);
  if (i > 0 && process.argv[i + 1]) return process.argv[i + 1];
  if (fallback === undefined) {
    console.error(`missing --${name}`);
    process.exit(2);
  }
  return fallback;
}

const dir = arg("dir");
const version = arg("version");
const tag = arg("tag");
const repo = arg("repo", "ReimagineMedias/unflub");
const notes = arg("notes", `See https://github.com/${repo}/releases/tag/${tag}`);

const ARCH = { x64: "x86_64", amd64: "x86_64", x86_64: "x86_64", arm64: "aarch64", aarch64: "aarch64", x86: "i686" };

/** Platform keys for one bundle file name, specific installer key first. */
function keysFor(name) {
  let m;
  if ((m = /_(x64|arm64|x86)-setup\.exe$/.exec(name))) return [`windows-${ARCH[m[1]]}-nsis`, `windows-${ARCH[m[1]]}`];
  if ((m = /_(x64|arm64|x86)_[A-Za-z-]+\.msi$/.exec(name))) return [`windows-${ARCH[m[1]]}-msi`];
  if ((m = /_(amd64|aarch64|x86_64)\.AppImage$/.exec(name))) return [`linux-${ARCH[m[1]]}-appimage`, `linux-${ARCH[m[1]]}`];
  if ((m = /_(amd64|arm64)\.deb$/.exec(name))) return [`linux-${ARCH[m[1]]}-deb`];
  if ((m = /\.(x86_64|aarch64)\.rpm$/.exec(name))) return [`linux-${ARCH[m[1]]}-rpm`];
  if ((m = /_(universal|aarch64|x64)\.app\.tar\.gz$/.exec(name))) {
    const arches = m[1] === "universal" ? ["x86_64", "aarch64"] : [ARCH[m[1]]];
    return arches.flatMap((a) => [`darwin-${a}-app`, `darwin-${a}`]);
  }
  return null;
}

const files = readdirSync(dir);
const platforms = {};
for (const sig of files.filter((f) => f.endsWith(".sig")).sort()) {
  const bundle = sig.slice(0, -".sig".length);
  if (!files.includes(bundle)) throw new Error(`${sig} has no ${bundle} beside it`);
  const keys = keysFor(bundle);
  if (!keys) throw new Error(`don't know which platform ${bundle} is for`);
  const entry = {
    signature: readFileSync(join(dir, sig), "utf8").trim(),
    url: `https://github.com/${repo}/releases/download/${tag}/${encodeURIComponent(bundle)}`,
  };
  for (const key of keys) {
    // The plain <os>-<arch> key goes to the first installer that claims it (NSIS before MSI is
    // guaranteed because MSI never claims it).
    if (key.split("-").length === 2 && platforms[key]) continue;
    if (platforms[key]) throw new Error(`two bundles for ${key}`);
    platforms[key] = entry;
  }
}

if (Object.keys(platforms).length === 0) throw new Error(`no .sig files in ${dir}`);
for (const key of Object.keys(platforms)) {
  const plain = key.split("-").slice(0, 2).join("-");
  if (!platforms[plain]) throw new Error(`${key} has no ${plain} fallback; add its setup.exe, AppImage or app bundle`);
}

const manifest = {
  version,
  notes,
  pub_date: new Date().toISOString().replace(/\.\d{3}Z$/, "Z"),
  platforms: Object.fromEntries(Object.entries(platforms).sort(([a], [b]) => a.localeCompare(b))),
};
process.stdout.write(JSON.stringify(manifest, null, 2) + "\n");
