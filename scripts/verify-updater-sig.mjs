#!/usr/bin/env node
// Checks updater signatures (`<file>.sig`) against the public key the app trusts
// (plugins.updater.pubkey in app/src-tauri/tauri.conf.json), so a release signed with the wrong
// private key fails instead of shipping updates every app would reject.
//
//   node scripts/verify-updater-sig.mjs <file> [<file> ...]
//
// The signatures are minisign's format, base64-encoded by Tauri. Only Node's own crypto is used
// (Ed25519, BLAKE2b-512), so nothing is downloaded in the job that holds the signing key.

import { createPublicKey, createHash, verify } from "node:crypto";
import { readFileSync } from "node:fs";
import { basename, dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");

const fail = (message) => {
  console.error(`verify-updater-sig: ${message}`);
  process.exit(1);
};

/** The base64 payload line of a minisign box (the line after the untrusted comment). */
function boxLines(text, what) {
  const lines = text.replace(/\r/g, "").split("\n");
  if (!lines[0]?.startsWith("untrusted comment:")) fail(`${what} is not a minisign box`);
  return lines;
}

const conf = JSON.parse(readFileSync(join(root, "app/src-tauri/tauri.conf.json"), "utf8"));
const pubkeyText = Buffer.from(conf.plugins?.updater?.pubkey ?? "", "base64").toString("utf8");
const pk = Buffer.from(boxLines(pubkeyText, "the public key")[1] ?? "", "base64");
if (pk.length !== 42 || pk.subarray(0, 2).toString() !== "Ed") fail("the public key is not an Ed25519 minisign key");
const keyId = pk.subarray(2, 10);
const publicKey = createPublicKey({
  key: { kty: "OKP", crv: "Ed25519", x: pk.subarray(10).toString("base64url") },
  format: "jwk",
});

const files = process.argv.slice(2);
if (files.length === 0) fail("usage: node scripts/verify-updater-sig.mjs <file> [<file> ...]");

for (const file of files) {
  const name = basename(file);
  const lines = boxLines(Buffer.from(readFileSync(`${file}.sig`, "utf8").trim(), "base64").toString("utf8"), `${name}.sig`);
  const sig = Buffer.from(lines[1] ?? "", "base64");
  const trusted = lines[2] ?? "";
  const globalSig = Buffer.from(lines[3] ?? "", "base64");
  if (sig.length !== 74 || !trusted.startsWith("trusted comment: ") || globalSig.length !== 64) {
    fail(`${name}.sig is not a minisign signature`);
  }
  const algorithm = sig.subarray(0, 2).toString();
  if (!sig.subarray(2, 10).equals(keyId)) {
    fail(`${name} was signed with key ${sig.subarray(2, 10).toString("hex")}, the app trusts ${keyId.toString("hex")}`);
  }
  const data = readFileSync(file);
  // "ED" signs the BLAKE2b-512 hash of the file (what Tauri makes), "Ed" the file itself.
  const message =
    algorithm === "ED" ? createHash("blake2b512").update(data).digest() : algorithm === "Ed" ? data : null;
  if (!message) fail(`${name}.sig uses an unknown algorithm`);
  const signature = sig.subarray(10);
  if (!verify(null, message, publicKey, signature)) fail(`${name}: the signature doesn't match the file`);
  const comment = Buffer.from(trusted.slice("trusted comment: ".length), "utf8");
  if (!verify(null, Buffer.concat([signature, comment]), publicKey, globalSig)) {
    fail(`${name}: the trusted comment's signature doesn't match`);
  }
  console.log(`${name}: signature valid (${comment.toString()})`);
}
