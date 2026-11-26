import { isDictionaryToken, trimWord, wordKey } from "./dictionary.ts";
import { LIMITS, type Settings } from "./types.ts";

export interface Additions {
  words: string[];
  shortcuts: Settings["userRules"];
}

export type Format = "text" | "json";
type Rejection = "invalid" | "existing" | "duplicate" | "full";
type Entry = { kind: "word" | "shortcut"; value: unknown; label: string };
type Input = { entries: Entry[] } | { error: "size" | "syntax" };

// Enough for a full dictionary even if every character needs a JSON escape.
export const MAX_IMPORT_SIZE = 6 * (LIMITS.words * LIMITS.word
  + LIMITS.rules * (LIMITS.word + LIMITS.replacement)) + 200_000;

const entry = (kind: Entry["kind"], value: unknown, label?: string): Entry => ({
  kind, value, label: (label ?? (typeof value === "string" ? value : "")).slice(0, 400),
});
const ruleLabel = (from: string, to: unknown) => typeof to === "string"
  ? `${from.slice(0, 128)} \u2192 ${to.slice(0, 272)}` : from;

export function readImport(text: string, format: Format): Input {
  if (text.length > MAX_IMPORT_SIZE) return { error: "size" };
  text = text.replace(/^\uFEFF/, "");
  if (format === "text") {
    return { entries: text.split(/\r\n?|\n/).flatMap((line, i) => {
      if (!trimWord(line)) return [];
      const tab = line.indexOf("\t");
      if (tab < 0) return [entry("word", trimWord(line))];
      const from = trimWord(line.slice(0, tab));
      const to = line.slice(tab + 1);
      if (!to.startsWith('"')) return [entry("shortcut", [from, to], ruleLabel(from, to))];
      try {
        const target: unknown = JSON.parse(to);
        return [entry("shortcut", [from, target], ruleLabel(from, target))];
      } catch (error) {
        if (!(error instanceof SyntaxError)) throw error;
        return [entry("shortcut", undefined, `${i + 1}: ${line}`)];
      }
    }) };
  }
  try {
    const data: unknown = JSON.parse(text);
    if (typeof data !== "object" || data === null || Array.isArray(data)
      || Object.keys(data).some((key) => key !== "words" && key !== "shortcuts")
      || (!("words" in data) && !("shortcuts" in data))) return { error: "syntax" };
    const words = "words" in data ? data.words : [];
    const shortcuts = "shortcuts" in data ? data.shortcuts : [];
    if (!Array.isArray(words) || !Array.isArray(shortcuts)) return { error: "syntax" };
    return { entries: [
      ...words.map((value: unknown) => entry("word", value)),
      ...shortcuts.map((value: unknown) => entry("shortcut", value,
        Array.isArray(value) && typeof value[0] === "string" ? ruleLabel(value[0], value[1]) : undefined)),
    ] };
  } catch (error) {
    if (!(error instanceof SyntaxError)) throw error;
    return { error: "syntax" };
  }
}

const length = (value: string) => Array.from(value).length;
const shortcut = (value: unknown): value is [string, string] => Array.isArray(value)
  && value.length === 2 && isDictionaryToken(value[0]) && typeof value[1] === "string"
  && /[^\p{White_Space}]/u.test(value[1]) && length(value[1]) <= LIMITS.replacement
  && !/[\p{Cc}\p{Cs}]/u.test(value[1].replace(/[\n\t]/g, ""));

export function planImport(input: Entry[] | Additions, settings: Settings) {
  const entries = Array.isArray(input) ? input : [
    ...input.words.map((value) => entry("word", value)),
    ...input.shortcuts.map((value) => entry("shortcut", value, value[0])),
  ];
  const additions: Additions = { words: [], shortcuts: [] };
  const rejected: { entry: Entry; reason: Rejection }[] = [];
  const existing = new Set([...settings.personalWords.map(wordKey), ...settings.userRules.map(([from]) => wordKey(from))]);
  const seen = new Set<string>();
  const check = (key: string, count: number, max: number) => {
    if (existing.has(key)) return "existing";
    if (seen.has(key)) return "duplicate";
    if (count >= max) return "full";
    seen.add(key);
  };
  for (const row of entries) {
    let reason: Rejection | undefined = "invalid";
    if (row.kind === "word" && isDictionaryToken(row.value)) {
      reason = check(wordKey(row.value), settings.personalWords.length + additions.words.length, LIMITS.words);
      if (!reason) additions.words.push(row.value);
    } else if (row.kind === "shortcut" && shortcut(row.value)) {
      reason = check(wordKey(row.value[0]), settings.userRules.length + additions.shortcuts.length, LIMITS.rules);
      if (!reason) additions.shortcuts.push(row.value);
    }
    if (reason) rejected.push({ entry: row, reason });
  }
  return { additions, rejected };
}

export function exportDictionary(settings: Settings, format: Format): string {
  const words = settings.personalWords;
  const shortcuts = settings.userRules;
  if (format === "json") return JSON.stringify({ words, shortcuts }, null, 2) + "\n";
  return [...words, ...shortcuts.map(([from, to]) =>
    `${from}\t${to.includes("\n") || to.startsWith('"') ? JSON.stringify(to) : to}`)].join("\n") + "\n";
}
