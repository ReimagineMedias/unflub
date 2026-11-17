#!/usr/bin/env node
// Keeps the app version identical everywhere it is written down.
//
//   node scripts/bump-version.mjs 0.2.0           set every version to 0.2.0
//   node scripts/bump-version.mjs --check         fail unless every file agrees
//   node scripts/bump-version.mjs --check 0.2.0   ...and agrees on 0.2.0 (the release workflow runs this)
//
// Files: Cargo.toml ([workspace.package]), Cargo.lock (workspace crates), app/src-tauri/tauri.conf.json,
// app/package.json and app/package-lock.json. Edits are textual so formatting stays as it is.

import { readFileSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const SEMVER = /^\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?(?:\+[0-9A-Za-z.-]+)?$/;

/** Each file: how to read its version(s) and how to write a new one. */
const files = [
  {
    path: "Cargo.toml",
    pattern: /(\[workspace\.package\][^[]*?\nversion = ")([^"]+)(")/,
  },
  {
    // Every workspace crate is named unflub*; registry crates have a `source` line, ours don't.
    path: "Cargo.lock",
    pattern: /(\[\[package\]\]\nname = "unflub[^"]*"\nversion = ")([^"]+)("\n(?!source))/g,
  },
  { path: "app/src-tauri/tauri.conf.json", pattern: /^(\s*"version": ")([^"]+)(")/m },
  { path: "app/package.json", pattern: /^(\s*"version": ")([^"]+)(")/m },
  {
    // The root project's version appears twice: at the top and under packages[""].
    path: "app/package-lock.json",
    pattern: /^((?: {2}| {6})"version": ")([^"]+)(")/gm,
    limit: 2,
  },
];

function versionsIn(file, text) {
  const re = new RegExp(file.pattern.source, file.pattern.flags.includes("g") ? file.pattern.flags : file.pattern.flags + "g");
  const found = [...text.matchAll(re)].map((m) => m[2]);
  return file.limit ? found.slice(0, file.limit) : found;
}

function setVersion(file, text, version) {
  let n = 0;
  const re = new RegExp(file.pattern.source, file.pattern.flags.includes("g") ? file.pattern.flags : file.pattern.flags + "g");
  return text.replace(re, (all, pre, _old, post) => {
    n += 1;
    return file.limit && n > file.limit ? all : `${pre}${version}${post}`;
  });
}

function usage(message) {
  if (message) console.error(message);
  console.error("usage: node scripts/bump-version.mjs <version> | --check [version]");
  process.exit(2);
}

const args = process.argv.slice(2);
const check = args[0] === "--check";
const wanted = (check ? args[1] : args[0])?.replace(/^v/, "");
if (!check && !wanted) usage();
if (wanted && !SEMVER.test(wanted)) usage(`not a semver version: ${wanted}`);

let ok = true;
const rows = [];
for (const file of files) {
  const full = join(root, file.path);
  const text = readFileSync(full, "utf8");
  const found = versionsIn(file, text);
  if (found.length === 0) {
    console.error(`${file.path}: no version found`);
    ok = false;
    continue;
  }
  if (check) {
    rows.push([file.path, [...new Set(found)].join(", ")]);
    continue;
  }
  writeFileSync(full, setVersion(file, text, wanted));
  rows.push([file.path, `${[...new Set(found)].join(", ")} -> ${wanted}`]);
}

for (const [path, v] of rows) console.log(`${path.padEnd(32)} ${v}`);

if (check) {
  const all = new Set(rows.flatMap(([, v]) => v.split(", ")));
  if (all.size !== 1) {
    console.error(`versions disagree: ${[...all].join(", ")}. Run: node scripts/bump-version.mjs <version>`);
    ok = false;
  } else if (wanted && !all.has(wanted)) {
    console.error(`expected ${wanted}, files say ${[...all][0]}`);
    ok = false;
  }
}
process.exit(ok ? 0 : 1);
