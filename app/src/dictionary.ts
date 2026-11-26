import { LIMITS } from "./types.ts";

export const trimWord = (word: string): string => word.replace(/^\p{White_Space}+|\p{White_Space}+$/gu, "");

/** Matches how the engine keys the personal dictionary and shortcut triggers. */
export const wordKey = (word: string): string => trimWord(word).toLowerCase().replace(/\u2019/g, "'");

export const isDictionaryToken = (value: unknown): value is string => typeof value === "string"
  && value.length > 0 && Array.from(value).length <= LIMITS.word
  && !/[\p{White_Space}\p{Cc}\p{Cs}]/u.test(value);
