import { Disclosure, Row, Segmented, Switch } from "./components";
import { t } from "./i18n";
import type { AppInfo, Settings } from "./types";

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
      <Disclosure />
      <div className="card">
        <Row label={t("privacy.stats")} hint={m?.disableUsageStats ? t("privacy.managed") : undefined}>
          <Switch label={t("privacy.stats")} disabled={m?.disableUsageStats}
            checked={s.shareUsageStats && !m?.disableUsageStats} onChange={(v) => update({ shareUsageStats: v })} />
        </Row>
        <Row label={t("privacy.crashes")} hint={m?.disableCrashReports ? t("privacy.managed") : undefined}>
          <Switch label={t("privacy.crashes")} disabled={m?.disableCrashReports}
            checked={s.sendCrashReports && !m?.disableCrashReports} onChange={(v) => update({ sendCrashReports: v })} />
        </Row>
      </div>

      <div className="section-title">{t("privacy.updates")}</div>
      <div className="card">
        <Row label={t("privacy.autoUpdate")} hint={m?.disableUpdates ? t("privacy.managed") : t("privacy.autoUpdate.hint")}>
          <Switch label={t("privacy.autoUpdate")} disabled={m?.disableUpdates}
            checked={s.autoUpdate && !m?.disableUpdates} onChange={(v) => update({ autoUpdate: v })} />
        </Row>
        <Row label={t("privacy.channel")} hint={t("privacy.channel.hint")}>
          <Segmented label={t("privacy.channel")} value={s.channel} onChange={(v) => update({ channel: v })} options={[
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
