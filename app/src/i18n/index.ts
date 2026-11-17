import { en, type StringKey } from "./en";

export type { StringKey };
export { previewScript } from "./en";

const strings: Record<StringKey, string> = en;

/** The string for `key`, with each `{name}` replaced by `vars.name`. */
export function t(key: StringKey, vars?: Record<string, string | number>): string {
  const s = strings[key];
  if (!vars) return s;
  return s.replace(/\{(\w+)\}/g, (m, name: string) => (name in vars ? String(vars[name]) : m));
}
