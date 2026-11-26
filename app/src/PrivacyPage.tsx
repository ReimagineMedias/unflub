import { usageReportPreview } from "./api";
import { CrashReportPreview, ReportPreview } from "./ReportPreview";
import { ExternalLink, Row, Segmented, Switch } from "./components";
import { t, type StringKey } from "./i18n";
import { LINKS } from "./links";
import type { AppInfo, Settings } from "./types";

/** What a usage report holds, one plain line per group (the groups of the report's contract). */
const SENT: [StringKey, StringKey][] = [
  ["privacy.sent.health", "privacy.sent.health.text"],
  ["privacy.sent.fixes", "privacy.sent.fixes.text"],
  ["privacy.sent.apps", "privacy.sent.apps.text"],
  ["privacy.sent.env", "privacy.sent.env.text"],
  ["privacy.sent.features", "privacy.sent.features.text"],
  ["privacy.sent.updates", "privacy.sent.updates.text"],
];

/** What leaves the computer, and the switches for it. */
export function PrivacyPage({ settings: s, info, update }: {
  settings: Settings;
  info: AppInfo | null;
  update: (patch: Partial<Settings>) => void;
}) {
  const m = info?.managed;

  return (
    <>
      <h1>{t("privacy.title")}</h1>
      <p className="lead">{t("privacy.lead")}</p>

      <div className="section-title">{t("privacy.sharing")}</div>
      <div className="disclosure">
        <p>{t("privacy.usage.lead")}</p>
        <p className="muted">{t("privacy.usage.never")} {t("privacy.usage.why")}</p>
      </div>
      <div className="card">
        <Row label={t("privacy.stats")} hint={m?.disableUsageStats ? t("privacy.managed") : t("privacy.stats.hint")}>
          <Switch label={t("privacy.stats")} disabled={m?.disableUsageStats}
            checked={s.shareUsageStats && !m?.disableUsageStats} onChange={(v) => update({ shareUsageStats: v })} />
        </Row>
        <Row label={t("privacy.crashes")} hint={m?.disableCrashReports ? t("privacy.managed") : t("privacy.crashes.hint")}>
          <Switch label={t("privacy.crashes")} disabled={m?.disableCrashReports}
            checked={s.sendCrashReports && !m?.disableCrashReports} onChange={(v) => update({ sendCrashReports: v })} />
        </Row>
      </div>

      <div className="section-title">{t("privacy.sent")}</div>
      <div className="card">
        <ul className="sent">
          {SENT.map(([label, text]) => (
            <li key={label}><strong>{t(label)}:</strong> {t(text)}</li>
          ))}
        </ul>
      </div>
      <div className="report-actions">
        <ReportPreview label={t("privacy.preview.today")} load={async () => {
          const { stats } = await usageReportPreview();
          return { json: stats, hint: t(stats ? "privacy.preview.today.hint" : "privacy.preview.off") };
        }} />
        <ReportPreview label={t("privacy.preview.last")} load={async () => {
          const { stats, lastStats } = await usageReportPreview();
          return { json: lastStats, hint: t(lastStats ? "privacy.preview.last.hint" : stats ? "privacy.preview.last.empty" : "privacy.preview.off") };
        }} />
        <CrashReportPreview label={t("privacy.preview.crash")} />
      </div>
      <p className="note"><ExternalLink href={LINKS.privacy}>{t("privacy.details")}</ExternalLink></p>

      <div className="section-title">{t("privacy.updates")}</div>
      <div className="card">
        <Row label={t("privacy.autoUpdate")} hint={m?.disableUpdates ? t("privacy.managed") : t("privacy.autoUpdate.hint")}>
          <Switch label={t("privacy.autoUpdate")} disabled={m?.disableUpdates}
            checked={s.autoUpdate && !m?.disableUpdates} onChange={(v) => update({ autoUpdate: v })} />
        </Row>
        <Row label={t("privacy.channel")} hint={m?.disableUpdates ? t("privacy.managed") : t("privacy.channel.hint")}>
          <Segmented label={t("privacy.channel")} disabled={m?.disableUpdates} value={s.channel} onChange={(v) => update({ channel: v })} options={[
            { value: "stable", label: t("privacy.channel.stable") }, { value: "beta", label: t("privacy.channel.beta") },
          ]} />
        </Row>
        <Row label={t("privacy.check")} hint={m?.disableUpdates ? t("privacy.managed") : t("privacy.check.hint")}>
          <span />
        </Row>
      </div>
    </>
  );
}
