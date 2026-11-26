import { useId, useRef, useState } from "react";
import { crashReportPreview } from "./api";
import { t } from "./i18n";

export function CrashReportPreview({ label }: { label: string }) {
  return <ReportPreview label={label} load={async () => {
    const { json, pending } = await crashReportPreview();
    return { json, hint: t(pending ? "privacy.preview.crash.pending" : "privacy.preview.crash.example") };
  }} />;
}

interface Report {
  json?: string | null;
  hint: string;
}

export function ReportPreview({ label, load }: {
  label: string;
  load: () => Promise<Report>;
}) {
  const dialog = useRef<HTMLDialogElement>(null);
  const request = useRef(0);
  const title = useId();
  const [report, setReport] = useState<Report | null>(null);

  const open = async () => {
    const id = ++request.current;
    setReport(null);
    dialog.current?.showModal();
    try {
      const next = await load();
      if (request.current === id) setReport(next);
    } catch {
      if (request.current === id) setReport({ hint: t("privacy.preview.error") });
    }
  };

  return (
    <>
      <button type="button" className="btn ghost" onClick={() => void open()}>{label}</button>
      <dialog ref={dialog} className="report-dialog" aria-labelledby={title} onClose={() => { request.current += 1; }}>
        <div className="report-preview">
          <div className="report-head">
            <h2 id={title}>{label}</h2>
            <form method="dialog"><button className="btn ghost">{t("privacy.preview.close")}</button></form>
          </div>
          <p role="status">{report?.hint ?? t("privacy.preview.loading")}</p>
          {report?.json && <pre tabIndex={0}>{report.json}</pre>}
        </div>
      </dialog>
    </>
  );
}
