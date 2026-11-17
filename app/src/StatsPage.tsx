import { useEffect, useState } from "react";
import { clearStats, statsView } from "./api";
import { Row, Sparkline, Switch } from "./components";
import { t } from "./i18n";
import type { Settings, StatsDay, StatsView } from "./types";

const nf = new Intl.NumberFormat("en");
const pct = (part: number, whole: number) =>
  whole > 0 ? `${(Math.round((part / whole) * 1000) / 10).toLocaleString("en")}%` : t("stats.none");

function Big({ label, day }: { label: string; day: StatsDay }) {
  return (
    <div className="big">
      <small>{label}</small>
      <strong>{nf.format(day.corrections)}</strong>
      <span>{t(day.corrections === 1 ? "stats.fixes.one" : "stats.fixes.many")}</span>
    </div>
  );
}

/** Local only: counts and the most fixed words, read from stats.json through the app. */
export function StatsPage({ settings, update }: { settings: Settings; update: (patch: Partial<Settings>) => void }) {
  const [view, setView] = useState<StatsView | null>(null);
  const [confirming, setConfirming] = useState(false);

  useEffect(() => {
    let live = true;
    const load = () => statsView().then((v) => live && setView(v));
    void load();
    // Fresh numbers while the page is open; nothing runs when it isn't.
    const timer = window.setInterval(load, 4000);
    return () => {
      live = false;
      window.clearInterval(timer);
    };
  }, []);

  if (!view) return null;
  const all = view.allTime;
  const peak = Math.max(0, ...view.last30);

  return (
    <>
      <h1>{t("stats.title")}</h1>
      <p className="lead">{t("stats.lead")}</p>
      <div className="bigs">
        <Big label={t("stats.today")} day={view.today} />
        <Big label={t("stats.week")} day={view.week} />
        <Big label={t("stats.allTime")} day={all} />
      </div>

      <div className="card chart-card">
        <div className="chart-head">
          <span>{t("stats.chart")}</span>
          {peak > 0 && <small>{t("stats.chart.peak", { n: nf.format(peak) })}</small>}
        </div>
        <Sparkline values={view.last30} label={t("stats.chart.label")} />
      </div>

      <div className="card" style={{ marginTop: 14 }}>
        <Row label={t("stats.fixedShare")} hint={t("stats.fixedShare.hint", { fixed: nf.format(all.corrections), checked: nf.format(all.wordsChecked) })}>
          <span className="figure">{pct(all.corrections, all.wordsChecked)}</span>
        </Row>
        <Row label={t("stats.undoRate")} hint={t("stats.undoRate.hint")}>
          <span className="figure">{pct(all.undos, all.corrections)}</span>
        </Row>
        <Row label={t("stats.saved")} hint={t("stats.saved.hint")}>
          <span className="figure">{nf.format(all.keystrokesSaved)}</span>
        </Row>
      </div>

      <div className="section-title">{t("stats.top")}</div>
      <div className="card">
        {!settings.statsKeepWords ? (
          <div className="row"><span className="empty">{t("stats.top.off")}</span></div>
        ) : view.topWords.length === 0 ? (
          <div className="row"><span className="empty">{t("stats.top.empty")}</span></div>
        ) : (
          <ol className="top-words">
            {view.topWords.map((w) => (
              <li key={`${w.typo}→${w.fix}`}>
                <s>{w.typo}</s>
                <span className="arrow">→</span>
                <span className="fix">{w.fix}</span>
                <span className="count">{t("stats.top.count", { n: nf.format(w.count) })}</span>
                <i style={{ width: `${(w.count / view.topWords[0].count) * 100}%` }} />
              </li>
            ))}
          </ol>
        )}
      </div>

      <div className="card" style={{ marginTop: 14 }}>
        <Row label={t("stats.keepWords")} hint={t("stats.keepWords.hint")}>
          <Switch label={t("stats.keepWords")} checked={settings.statsKeepWords} onChange={(v) => update({ statsKeepWords: v })} />
        </Row>
        <Row label={t("stats.clear")} hint={t("stats.clear.hint")}>
          <button className="btn ghost" onBlur={() => setConfirming(false)} onClick={() => {
            if (!confirming) return setConfirming(true);
            setConfirming(false);
            void clearStats().then(setView);
          }}>
            {confirming ? t("stats.clear.confirm") : t("stats.clear.button")}
          </button>
        </Row>
      </div>
    </>
  );
}
