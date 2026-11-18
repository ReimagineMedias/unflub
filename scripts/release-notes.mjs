#!/usr/bin/env node
// Release notes from Conventional Commits since the previous release tag, as Markdown on stdout.
//
//   node scripts/release-notes.mjs [--to <ref>] [--repo owner/name] [--version x.y.z]
//
// --to       last commit to include (default HEAD; the release workflow passes the tag)
// --repo     for commit and compare links (default ReimagineMedias/unflub)
// --version  for installer names (default: app/src-tauri/tauri.conf.json)
//
// feat, fix and perf are listed; a `!` or a BREAKING CHANGE footer goes under "Breaking changes".
// Housekeeping (refactor, docs, ci, build, test, chore, style) is counted, not listed.

import { execFileSync } from "node:child_process";
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");

function arg(name, fallback) {
  const i = process.argv.indexOf(`--${name}`);
  return i > 0 && process.argv[i + 1] ? process.argv[i + 1] : fallback;
}

const to = arg("to", "HEAD");
const repo = arg("repo", "ReimagineMedias/unflub");
const version =
  arg("version") ?? JSON.parse(readFileSync(join(root, "app/src-tauri/tauri.conf.json"), "utf8")).version;

const git = (...args) =>
  execFileSync("git", args, { cwd: root, encoding: "utf8", stdio: ["ignore", "pipe", "ignore"] }).trim();

/**
 * The newest release tag before `to`, or null for the first release. A stable release is compared with the
 * previous stable one, so its notes cover every beta in between.
 */
function previousTag() {
  const skipBetas = version.includes("-") ? [] : ["--exclude", "v*-*"];
  try {
    return git("describe", "--tags", "--abbrev=0", "--match", "v[0-9]*.[0-9]*.[0-9]*", ...skipBetas, `${to}^`);
  } catch {
    return null;
  }
}

const SECTIONS = [
  ["breaking", "Breaking changes"],
  ["feat", "New"],
  ["fix", "Fixes"],
  ["perf", "Faster"],
  ["other", "Other changes"],
];
const QUIET = new Set(["refactor", "docs", "ci", "build", "test", "chore", "style", "revert"]);
const HEADER = /^(\w+)(?:\(([^)]+)\))?(!)?: (.+)$/;

const prev = previousTag();
const range = prev ? `${prev}..${to}` : to;
const log = git("log", range, "--no-merges", "--format=%H%x1f%s%x1f%b%x1e");

const groups = Object.fromEntries(SECTIONS.map(([k]) => [k, []]));
let quiet = 0;
for (const entry of log.split("\x1e")) {
  const [sha, subject, body = ""] = entry.trim().split("\x1f");
  if (!sha) continue;
  const link = `[${sha.slice(0, 7)}](https://github.com/${repo}/commit/${sha})`;
  const m = HEADER.exec(subject);
  if (!m) {
    groups.other.push(`- ${subject} (${link})`);
    continue;
  }
  const [, type, scope, bang, text] = m;
  const breaking = Boolean(bang) || /^BREAKING[ -]CHANGE:/m.test(body);
  const line = `- ${scope ? `**${scope}:** ` : ""}${text} (${link})`;
  if (breaking) groups.breaking.push(line);
  else if (groups[type]) groups[type].push(line);
  else if (QUIET.has(type)) quiet += 1;
  else groups.other.push(line);
}

const out = [];
for (const [key, title] of SECTIONS) {
  if (groups[key].length) out.push(`## ${title}`, "", ...groups[key], "");
}
if (out.length === 0) out.push("Maintenance release.", "");
if (quiet) out.push(`Plus ${quiet} internal change${quiet === 1 ? "" : "s"} (refactoring, docs, CI).`, "");

out.push(
  "## Install",
  "",
  `- **Windows:** \`unflub_${version}_x64-setup.exe\` (\`_arm64-setup.exe\` on ARM). Installs for your account only, no admin needed. unflub updates itself from then on.`,
  ...(version.includes("-") ? [] : [`- **Managed installs:** \`unflub_${version}_x64_en-US.msi\` installs for all users.`]),
  "- The installers aren't code-signed yet, so Windows SmartScreen may say \"Windows protected your PC\". Choose **More info**, then **Run anyway**. Compare the file with `SHA256SUMS.txt` if you want to be sure it is ours.",
  "",
);
if (prev) out.push(`**Full changelog:** https://github.com/${repo}/compare/${prev}...v${version}`, "");

process.stdout.write(out.join("\n"));
