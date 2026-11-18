#!/usr/bin/env node
// Prepares a release pull request: bumps every version, adds the CHANGELOG.md section and commits
// `chore(release): <version>` on a new branch `release/v<version>`. Squash-merging that PR (keep its
// title) makes .github/workflows/release-tag.yml tag the merge commit and start the Release workflow.
//
//   node scripts/prepare-release.mjs 0.2.0              stable channel
//   node scripts/prepare-release.mjs 0.2.0-beta.1       beta channel, GitHub pre-release, setup.exe only
//   node scripts/prepare-release.mjs 0.2.0 --pr         ...then push the branch and open the PR (needs gh)
//
// Versions are SemVer. Before 1.0: a new feature or a breaking change bumps the minor (0.2.0), a fix
// bumps the patch (0.1.1). Betas of the next version count up from 1 (0.2.0-beta.1, 0.2.0-beta.2), and
// the stable 0.2.0 follows them. See docs/releasing.md.

import { execFileSync } from "node:child_process";
import { existsSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const REPO = "ReimagineMedias/unflub";
const VERSION = /^(\d+)\.(\d+)\.(\d+)(?:-beta\.(\d+))?$/;

const run = (cmd, args, opts = {}) =>
  execFileSync(cmd, args, { cwd: root, encoding: "utf8", stdio: ["ignore", "pipe", "inherit"], ...opts }).trim();
const git = (...args) => run("git", args);
const fail = (message) => {
  console.error(`prepare-release: ${message}`);
  process.exit(1);
};

const args = process.argv.slice(2);
const version = args.find((a) => !a.startsWith("--"));
const openPr = args.includes("--pr");
if (!version) fail("usage: node scripts/prepare-release.mjs <x.y.z | x.y.z-beta.n> [--pr]");
const parsed = VERSION.exec(version);
if (!parsed) fail(`${version} is not x.y.z or x.y.z-beta.n`);

/** Compares two versions this script accepts; a beta sorts before its stable release. */
function compare(a, b) {
  const pa = VERSION.exec(a);
  const pb = VERSION.exec(b);
  for (let i = 1; i <= 3; i += 1) {
    if (Number(pa[i]) !== Number(pb[i])) return Number(pa[i]) - Number(pb[i]);
  }
  const ba = pa[4] === undefined ? Infinity : Number(pa[4]);
  const bb = pb[4] === undefined ? Infinity : Number(pb[4]);
  return ba === bb ? 0 : ba < bb ? -1 : 1;
}

const current = JSON.parse(readFileSync(join(root, "app/src-tauri/tauri.conf.json"), "utf8")).version;
const tags = git("tag", "--list", "v*").split("\n").filter(Boolean);
const released = tags.map((t) => t.slice(1)).filter((v) => VERSION.test(v));
const newest = released.sort(compare).at(-1);
if (newest && compare(version, newest) <= 0) fail(`${version} is not newer than the last release ${newest}`);
if (!newest && VERSION.test(current) && compare(version, current) < 0) fail(`${version} is older than the app's ${current}`);

// A clean main that matches origin, so the release PR holds nothing but the release.
if (git("status", "--porcelain")) fail("the working tree has changes; commit or stash them first");
git("fetch", "--quiet", "--tags", "origin");
if (git("ls-remote", "--tags", "origin", `refs/tags/v${version}`)) fail(`v${version} already exists on origin`);
if (git("rev-parse", "--abbrev-ref", "HEAD") !== "main") fail("run this on main");
if (git("rev-parse", "HEAD") !== git("rev-parse", "origin/main")) fail("main is not the same as origin/main; pull first");

const branch = `release/v${version}`;
git("switch", "--quiet", "-c", branch);
run("node", ["scripts/bump-version.mjs", version], { stdio: "inherit" });

// The changelog section: the release notes without the per-release install instructions,
// one heading level down.
const notes = run("node", ["scripts/release-notes.mjs", "--version", version, "--repo", REPO]);
const body = notes
  .split(/\n(?=## )/)
  .filter((part) => !part.startsWith("## Install"))
  .join("\n")
  .replace(/^## /gm, "### ")
  .trim();
const date = new Date().toISOString().slice(0, 10);
const heading = `## [${version}](https://github.com/${REPO}/releases/tag/v${version}) - ${date}`;
const intro =
  "# Changelog\n\nWhat changed in each unflub release, newest first. Generated from the Conventional Commit\n" +
  "titles of merged pull requests by `scripts/prepare-release.mjs`.\n";
const path = join(root, "CHANGELOG.md");
const old = existsSync(path) ? readFileSync(path, "utf8").replace(/^# Changelog[\s\S]*?\n(?=## )/, "") : "";
writeFileSync(path, `${intro}\n${heading}\n\n${body}\n${old ? `\n${old.trimStart()}` : ""}`);

const title = `chore(release): ${version}`;
git("add", "-A", "--", "CHANGELOG.md", "Cargo.toml", "Cargo.lock", "app/src-tauri/tauri.conf.json", "app/package.json", "app/package-lock.json");
git("commit", "--quiet", "-m", title);
console.log(`Committed "${title}" on ${branch}.`);

const channel = version.includes("-") ? "beta (GitHub pre-release, setup.exe only)" : "stable";
const prBody = [
  "**Problem**",
  `Release ${version} on the ${channel} channel.`,
  "",
  "**Change**",
  "- Every version location set to " + `\`${version}\`` + " (`node scripts/bump-version.mjs --check` agrees).",
  "- CHANGELOG.md section from the merged pull requests since the last release.",
  "",
  "> [!IMPORTANT]",
  "> Squash-merge with this title. The merge commit is then tagged `v" + version + "` and the Release workflow",
  "> builds, drafts and registers it paused at 0%. Publishing and the rollout stay manual (docs/releasing.md).",
  "",
  "<details><summary>Changelog section</summary>",
  "",
  body,
  "",
  "</details>",
].join("\n");

if (openPr) {
  git("push", "--quiet", "-u", "origin", branch);
  const url = run("gh", ["pr", "create", "--base", "main", "--head", branch, "--title", title, "--body", prBody]);
  console.log(url);
} else {
  console.log(`Next: git push -u origin ${branch}, then open a PR titled "${title}" (or rerun with --pr).`);
}
