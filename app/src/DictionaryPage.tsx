import { useState } from "react";
import { ChipList, Row, RuleList } from "./components";
import { isDictionaryToken, trimWord, wordKey } from "./dictionary";
import { DictionaryTransfer } from "./DictionaryTransfer";
import { t } from "./i18n";
import { LIMITS, type Settings } from "./types";

const byName = new Intl.Collator(undefined, { sensitivity: "base" });
const date = new Intl.DateTimeFormat(undefined, { dateStyle: "medium", timeZone: "UTC" });

export function DictionaryPage({ settings, update, flush, onImported }: {
  settings: Settings;
  update: (patch: Partial<Settings>, later?: boolean) => void;
  flush: () => Promise<boolean>;
  onImported: (settings: Settings) => void;
}) {
  const [search, setSearch] = useState("");
  const filter = wordKey(search);
  const words = settings.personalWords;
  const visible = words.filter((word) => wordKey(word).includes(filter)).sort(byName.compare);
  const describe = (word: string) => {
    const day = settings.wordSources?.[wordKey(word)];
    if (day === null) return t("dictionary.words.added");
    if (typeof day !== "number" || !Number.isFinite(day) || Math.abs(day) > 100_000_000) return t("dictionary.words.unknown");
    return t("dictionary.words.learned", { date: date.format(day * 86_400_000) });
  };
  return (
    <>
      <h1>{t("dictionary.title")}</h1>
      <p className="lead">{t("dictionary.lead")}</p>
      <div className="card">
        <Row stack label={t("dictionary.words")} hint={t("dictionary.words.hint")}>
          <div className="add">
            <input type="search" value={search} onChange={(event) => setSearch(event.currentTarget.value)}
              aria-label={t("dictionary.words.search")} placeholder={t("dictionary.words.search")} />
          </div>
          <ChipList items={words} visibleItems={visible} describe={describe} onChange={(v) => update({ personalWords: v })}
            placeholder={t("dictionary.words.placeholder")} empty={t(filter ? "dictionary.words.noMatches" : "dictionary.words.empty")}
            normalise={trimWord} single={t("dictionary.words.single")} maxLength={LIMITS.word * 2}
            validate={(value) => !isDictionaryToken(value) ? t("dictionary.words.invalid")
              : words.some((word) => wordKey(word) === wordKey(value)) ? t("dictionary.words.duplicate")
              : words.length >= LIMITS.words ? t("dictionary.words.limit") : undefined} />
        </Row>
        <Row stack label={t("dictionary.shortcuts")} hint={t("dictionary.shortcuts.hint")}>
          <RuleList rules={settings.userRules} onChange={(v, typing) => update({ userRules: v }, typing)} onCommit={flush} />
        </Row>
      </div>
      <DictionaryTransfer settings={settings} flush={flush} onImported={onImported} />
    </>
  );
}
