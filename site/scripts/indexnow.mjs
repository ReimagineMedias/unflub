// Tells search engines that support IndexNow (Bing, Yandex and others) that every page in the sitemap changed.
// Run after a production deploy: node scripts/indexnow.mjs [host] [--dry-run]. Reads dist/sitemap-0.xml from the
// build. --dry-run prints the request instead of sending it.
// The key file public/<key>.txt is public by design; it proves we own the host.
import { appendFileSync, readdirSync, readFileSync } from "node:fs";

const args = process.argv.slice(2);
const dryRun = args.includes("--dry-run");
const host = args.find((a) => !a.startsWith("--")) ?? "unflub.rmgn.net";
const publicDir = new URL("../public/", import.meta.url);
const keyFile = readdirSync(publicDir).find((f) => /^[0-9a-f]{32}\.txt$/.test(f));
if (!keyFile) throw new Error("No IndexNow key file (public/<32 hex>.txt)");
const key = keyFile.slice(0, -4);

const sitemap = readFileSync(new URL("../dist/sitemap-0.xml", import.meta.url), "utf8");
const urlList = [...sitemap.matchAll(/<loc>([^<]+)<\/loc>/g)]
  .map((m) => m[1])
  .filter((u) => new URL(u).host === host);
if (urlList.length === 0) throw new Error(`No ${host} URLs in dist/sitemap-0.xml`);

const body = { host, key, keyLocation: `https://${host}/${key}.txt`, urlList };
if (dryRun) {
  console.log(JSON.stringify(body, null, 2));
  process.exit(0);
}
const res = await fetch("https://api.indexnow.org/indexnow", {
  method: "POST",
  headers: { "Content-Type": "application/json; charset=utf-8" },
  body: JSON.stringify(body),
});
console.log(`IndexNow: ${res.status} for ${urlList.length} URLs`);
// In GitHub Actions, hand the status code to the job summary.
if (process.env["GITHUB_OUTPUT"]) appendFileSync(process.env["GITHUB_OUTPUT"], `status=${res.status}
urls=${urlList.length}
`);
// 200 accepted, 202 accepted pending key check. Anything else is reported, not fatal to the deploy.
if (res.status !== 200 && res.status !== 202) {
  console.log((await res.text()).slice(0, 500));
  process.exitCode = 1;
}
