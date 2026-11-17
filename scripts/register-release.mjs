#!/usr/bin/env node
// Writes the body of `POST /unflub/v1/admin/releases` from the signed bundles in one folder.
//
//   node scripts/register-release.mjs --dir <assets> --version 0.2.0 [--notes-file release-notes.md] [--list]
//
// One platform per architecture, keyed like the updater (`windows-x86_64`): `file` is the NSIS
// setup the updater runs, `signature` its .sig, `installer` what the website serves (the same
// setup). Pre-release versions (0.3.0-beta.1) go to the beta channel. The API registers every
// release paused at 0% (docs/releasing.md).
//
// --list prints the files to upload to R2 instead, one per line: installers and their .sig only.

import { readdirSync, readFileSync } from "node:fs";
import { join } from "node:path";

function arg(name, fallback) {
  const i = process.argv.indexOf(`--${name}`);
  if (i > 0 && process.argv[i + 1] && !process.argv[i + 1].startsWith("--")) return process.argv[i + 1];
  if (fallback === undefined) {
    console.error(`missing --${name}`);
    process.exit(2);
  }
  return fallback;
}

const dir = arg("dir");
const version = arg("version");
const notesFile = arg("notes-file", "");
const files = readdirSync(dir).sort();

/** Installers and updater signatures: the only files that go to R2. */
const UPLOADABLE = /(-setup\.exe|\.msi|\.dmg|\.app\.tar\.gz|\.AppImage|\.deb|\.rpm)(\.sig)?$/;

if (process.argv.includes("--list")) {
  const list = files.filter((f) => UPLOADABLE.test(f));
  if (list.length === 0) throw new Error(`no installers in ${dir}`);
  process.stdout.write(list.join("\n") + "\n");
  process.exit(0);
}

const ARCH = { x64: "x86_64", arm64: "aarch64" };
const platforms = {};
for (const file of files) {
  const m = /_(x64|arm64)-setup\.exe$/.exec(file);
  if (!m) continue;
  if (!files.includes(`${file}.sig`)) throw new Error(`${file} has no .sig beside it`);
  platforms[`windows-${ARCH[m[1]]}`] = {
    file,
    signature: readFileSync(join(dir, `${file}.sig`), "utf8").trim(),
    installer: file,
  };
}
if (Object.keys(platforms).length === 0) throw new Error(`no setup.exe in ${dir}`);

// The API keeps notes short enough for the About page; the full notes are on GitHub.
let notes = notesFile ? readFileSync(notesFile, "utf8").trim() : "";
if (notes.length > 4000) notes = notes.slice(0, 3990).trimEnd() + "\n…";

const body = {
  version,
  channel: version.includes("-") ? "beta" : "stable",
  notes,
  pub_date: new Date().toISOString().replace(/\.\d{3}Z$/, "Z"),
  platforms,
};
process.stdout.write(JSON.stringify(body, null, 2) + "\n");
