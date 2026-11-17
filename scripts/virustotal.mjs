#!/usr/bin/env node
// Scans release installers with VirusTotal and fails when more than --max engines flag one.
//
//   VT_API_KEY=… node scripts/virustotal.mjs --max 2 file…
//
// Each file is uploaded (large ones through an upload URL), then the analysis is polled until
// it completes. Writes a Markdown table to $GITHUB_STEP_SUMMARY when set. A new unsigned
// installer is often flagged by a few heuristic engines; above the limit the release stops so
// someone looks first (docs/releasing.md, "Antivirus checks").

import { appendFileSync, readFileSync, statSync } from "node:fs";
import { basename } from "node:path";

const key = process.env.VT_API_KEY;
if (!key) {
  console.error("VT_API_KEY is not set");
  process.exit(2);
}
const args = process.argv.slice(2);
const maxIndex = args.indexOf("--max");
const max = maxIndex >= 0 ? Number(args.splice(maxIndex, 2)[1]) : 2;
const files = args;
if (files.length === 0) {
  console.error("no files given");
  process.exit(2);
}

const api = "https://www.virustotal.com/api/v3";
const headers = { "x-apikey": key };
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

async function json(res) {
  if (!res.ok) throw new Error(`${res.status} ${await res.text()}`);
  return res.json();
}

async function upload(file) {
  const size = statSync(file).size;
  let url = `${api}/files`;
  if (size > 32 * 1024 * 1024) {
    url = (await json(await fetch(`${api}/files/upload_url`, { headers }))).data;
  }
  const form = new FormData();
  form.append("file", new Blob([readFileSync(file)]), basename(file));
  const res = await json(await fetch(url, { method: "POST", headers, body: form }));
  return res.data.id;
}

async function analyse(id) {
  // Free keys allow 4 requests a minute; poll gently, for up to 20 minutes.
  for (let i = 0; i < 40; i++) {
    const res = await json(await fetch(`${api}/analyses/${id}`, { headers }));
    if (res.data.attributes.status === "completed") return res;
    await sleep(30_000);
  }
  throw new Error(`analysis ${id} did not finish in time`);
}

const rows = [];
let failed = false;
for (const file of files) {
  const id = await upload(file);
  const res = await analyse(id);
  const stats = res.data.attributes.stats;
  const flagged = (stats.malicious ?? 0) + (stats.suspicious ?? 0);
  const sha = res.meta?.file_info?.sha256 ?? "";
  const ok = flagged <= max;
  failed ||= !ok;
  rows.push(`| \`${basename(file)}\` | ${flagged} | ${ok ? "✅" : "❌"} | [report](https://www.virustotal.com/gui/file/${sha}) |`);
  console.log(`${basename(file)}: ${flagged} engines flagged it (limit ${max})`);
}

const summary = [
  "### VirusTotal",
  "",
  `| File | Engines flagging | ≤ ${max} | Report |`,
  "|---|---|---|---|",
  ...rows,
  "",
].join("\n");
if (process.env.GITHUB_STEP_SUMMARY) appendFileSync(process.env.GITHUB_STEP_SUMMARY, summary + "\n");
if (failed) {
  console.error(`::error::More than ${max} antivirus engines flagged an installer. See the run summary.`);
  process.exit(1);
}
