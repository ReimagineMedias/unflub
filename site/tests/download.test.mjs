// The download flow's pure parts (src/scripts/download-core.ts), run directly with Node's type stripping.
import { test } from "node:test";
import assert from "node:assert/strict";
import {
  TimeoutError,
  downloadUrl,
  isDownloadHref,
  pickRef,
  withDeadline,
} from "../src/scripts/download-core.ts";

const API = "https://api.rmgn.net/unflub/v1/download/windows";
const known = new Set(["site", "github", "hn", "reddit"]);

test("ref comes from the landing URL, then this visit, then falls back to site", () => {
  assert.equal(pickRef("?ref=hn", null, known), "hn");
  assert.equal(pickRef("?ref=bogus", "reddit", known), "reddit");
  assert.equal(pickRef("", "bogus", known), "site");
  // One decoded value, not extra parameters: the injection stays out.
  assert.equal(pickRef("?ref=hn%26t%3Devil", null, known), "site");
});

test("the download URL encodes ref and token as single values", () => {
  const url = new URL(downloadUrl(API, "hn", "abc&next=https://evil.example"));
  assert.equal(url.origin + url.pathname, API);
  assert.equal(url.searchParams.get("ref"), "hn");
  assert.equal(url.searchParams.get("t"), "abc&next=https://evil.example");
  assert.equal(url.searchParams.get("next"), null);
  assert.equal(new URL(downloadUrl(API, "site")).searchParams.has("t"), false);
});

test("only the exact endpoint counts as a download link", () => {
  assert.ok(isDownloadHref(`${API}?ref=site`, API));
  assert.ok(!isDownloadHref("https://api.rmgn.net/unflub/v1/download/windows-evil", API));
  assert.ok(!isDownloadHref("https://api.rmgn.net.evil.example/unflub/v1/download/windows", API));
  assert.ok(!isDownloadHref("/download/", API));
});

test("a stalled check times out instead of hanging", async () => {
  const never = new Promise(() => undefined);
  await assert.rejects(withDeadline(never, 20, "check").promise, TimeoutError);
  assert.equal(await withDeadline(Promise.resolve("token"), 1000, "check").promise, "token");
});

test("stopping the deadline lets an interactive check take its time", async () => {
  const slow = new Promise((r) => setTimeout(() => r("token"), 60));
  const d = withDeadline(slow, 20, "check");
  d.stop();
  assert.equal(await d.promise, "token");
});
