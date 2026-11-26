import assert from "node:assert/strict";
import { test } from "node:test";
import { exportDictionary, planImport, readImport } from "./dictionaryImport.ts";
import { defaultSettings, LIMITS } from "./types.ts";

function plan(text, format = "text", settings = defaultSettings) {
  const input = readImport(text, format);
  assert.ok("entries" in input);
  return planImport(input.entries, settings);
}

test("round trips words and shortcuts with tabs, quotes and newlines", () => {
  const words = ["Merci", "Port-au-Prince"];
  const shortcuts = [["tab", "one\ttwo"], ["quote", '"Hi"'], ["lines", "one\n two"]];
  for (const format of ["text", "json"]) {
    const text = exportDictionary({ ...defaultSettings, personalWords: words, userRules: shortcuts }, format);
    assert.deepEqual(plan(text, format), { additions: { words, shortcuts }, rejected: [] });
  }
});

test("previews normalized conflicts and invalid entries without touching saved entries", () => {
  const settings = { ...defaultSettings, personalWords: ["Tauri"], userRules: [["o’clock", "old"]] };
  const result = plan("\uFEFFtauri\r\nO'CLOCK\tnew\r\nNew\r\nNEW\r\na b\r\nfoo\t\"bad", "text", settings);
  assert.deepEqual(result.additions, { words: ["New"], shortcuts: [] });
  assert.deepEqual(result.rejected.map(({ reason }) => reason), ["existing", "existing", "duplicate", "invalid", "invalid"]);
  assert.deepEqual(settings.userRules, [["o’clock", "old"]]);
});

test("applies character and list limits and rejects bad JSON shapes", () => {
  const settings = {
    ...defaultSettings,
    personalWords: Array.from({ length: LIMITS.words - 1 }, (_, i) => `word${i}`),
    userRules: Array.from({ length: LIMITS.rules }, (_, i) => [`rule${i}`, "text"]),
  };
  const word = "𐐀".repeat(LIMITS.word);
  const result = plan(JSON.stringify({
    words: [word + "x", word, "extra", "\ud800"],
    shortcuts: [["brb", "back"], ["abc", "x".repeat(LIMITS.replacement + 1)]],
  }), "json", settings);
  assert.deepEqual(result.additions, { words: [word], shortcuts: [] });
  assert.deepEqual(result.rejected.map(({ reason }) => reason), ["invalid", "full", "invalid", "full", "invalid"]);
  assert.deepEqual(readImport('{"words":"no"}', "json"), { error: "syntax" });
});
