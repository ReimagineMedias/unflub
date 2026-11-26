import { useId, useRef, useState } from "react";
import { getSettings, importDictionary } from "./api";
import { Row } from "./components";
import { exportDictionary, MAX_IMPORT_SIZE, planImport, readImport, type Format } from "./dictionaryImport";
import { t, type StringKey } from "./i18n";
import type { Settings } from "./types";

const PAGE_SIZE = 20;

export function DictionaryTransfer({ settings, flush, onImported }: {
  settings: Settings;
  flush: () => Promise<boolean>;
  onImported: (settings: Settings) => void;
}) {
  const dialog = useRef<HTMLDialogElement>(null);
  const title = useId();
  const [format, setFormat] = useState<Format>("text");
  const [text, setText] = useState("");
  const [draft, setDraft] = useState<ReturnType<typeof planImport>>();
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<StringKey>();
  const [status, setStatus] = useState("");
  const [page, setPage] = useState(0);
  const resetPreview = () => { setDraft(undefined); setError(undefined); setPage(0); };

  const open = () => {
    resetPreview();
    setStatus("");
    setText("");
    setFormat("text");
    dialog.current?.showModal();
  };
  const loadFile = async (file: File) => {
    resetPreview();
    setBusy(true);
    try {
      const nextFormat = /\.json$/i.test(file.name) ? "json" : "text";
      setFormat(nextFormat);
      setText("");
      if (file.size > 4 * MAX_IMPORT_SIZE) {
        setError("dictionary.transfer.size");
        return;
      }
      const bytes = new Uint8Array(await file.arrayBuffer());
      const encoding = bytes[0] === 0xff && bytes[1] === 0xfe ? "utf-16le"
        : bytes[0] === 0xfe && bytes[1] === 0xff ? "utf-16be" : "utf-8";
      const content = new TextDecoder(encoding, { fatal: true }).decode(bytes);
      if (content.length > MAX_IMPORT_SIZE) setError("dictionary.transfer.size");
      else setText(content);
    } catch {
      setError("dictionary.transfer.readError");
    } finally {
      setBusy(false);
    }
  };
  const preview = async () => {
    resetPreview();
    const input = readImport(text, format);
    if ("error" in input) {
      setError(input.error === "size" ? "dictionary.transfer.size" : "dictionary.transfer.syntax");
      return;
    }
    setBusy(true);
    try {
      if (!await flush()) {
        setError("dictionary.transfer.settingsError");
        return;
      }
      setDraft(planImport(input.entries, await getSettings()));
    } catch {
      setError("dictionary.transfer.settingsError");
    } finally {
      setBusy(false);
    }
  };
  const apply = async () => {
    if (!draft) return;
    setBusy(true);
    try {
      if (!await flush()) {
        setDraft(undefined);
        setError("dictionary.transfer.settingsError");
        return;
      }
      onImported(await importDictionary(draft.additions));
      setStatus(t("dictionary.transfer.added", { n: count }));
      dialog.current?.close();
    } catch {
      setDraft(undefined);
      setError("dictionary.transfer.applyError");
    } finally {
      setBusy(false);
    }
  };
  const download = (kind: Format) => {
    const url = URL.createObjectURL(new Blob([exportDictionary(settings, kind)], {
      type: kind === "json" ? "application/json" : "text/plain;charset=utf-8",
    }));
    const link = document.createElement("a");
    link.href = url;
    link.download = kind === "json" ? "unflub-dictionary.json" : "unflub-dictionary.txt";
    link.click();
    window.setTimeout(() => URL.revokeObjectURL(url), 1000);
  };

  const words = draft?.additions.words ?? [];
  const shortcuts = draft?.additions.shortcuts ?? [];
  const count = words.length + shortcuts.length;
  const total = count + (draft?.rejected.length ?? 0);
  const first = page * PAGE_SIZE;
  const last = Math.min(first + PAGE_SIZE, total);

  return (
    <div className="card dictionary-transfer">
      <Row stack label={t("dictionary.transfer.title")} hint={t("dictionary.transfer.hint")}>
        <div className="report-actions">
          <button type="button" className="btn" onClick={open}>{t("dictionary.transfer.open")}</button>
          <button type="button" className="btn ghost" onClick={() => download("text")}>{t("dictionary.transfer.exportText")}</button>
          <button type="button" className="btn ghost" onClick={() => download("json")}>{t("dictionary.transfer.exportJson")}</button>
        </div>
        {status && <small role="status">{status}</small>}
      </Row>
      <dialog ref={dialog} className="report-dialog" aria-labelledby={title} onCancel={(event) => { if (busy) event.preventDefault(); }}>
        <div className="report-preview">
          <div className="report-head">
            <h2 id={title}>{t("dictionary.transfer.open")}</h2>
            <form method="dialog"><button className="btn ghost" disabled={busy}>{t("privacy.preview.close")}</button></form>
          </div>
          <div className="transfer-inputs">
            <label>
              {t("dictionary.transfer.format")}
              <select value={format} disabled={busy} onChange={(event) => {
                setFormat(event.currentTarget.value === "json" ? "json" : "text");
                resetPreview();
              }}>
                <option value="text">{t("dictionary.transfer.text")}</option>
                <option value="json">{t("dictionary.transfer.json")}</option>
              </select>
            </label>
            <label>
              {t("dictionary.transfer.file")}
              <input type="file" accept=".txt,.tsv,.json,text/plain,application/json" disabled={busy}
                onChange={(event) => {
                  const file = event.currentTarget.files?.[0];
                  event.currentTarget.value = "";
                  if (file) void loadFile(file);
                }} />
            </label>
            <label>
              {t("dictionary.transfer.paste")}
              <textarea value={text} disabled={busy} spellCheck={false} rows={4} onChange={(event) => {
                setText(event.currentTarget.value);
                resetPreview();
              }} />
            </label>
          </div>
          <p>{t(format === "text" ? "dictionary.transfer.textHint" : "dictionary.transfer.jsonHint")}</p>
          {error && <p role="alert">{t(error)}</p>}
          <button className="btn ghost" type="button" disabled={busy || !text.trim()} onClick={() => void preview()}>
            {t("dictionary.transfer.preview")}
          </button>
          {draft && (
            <>
              <p role="status">{t("dictionary.transfer.summary", {
                words: words.length, shortcuts: shortcuts.length, skipped: draft.rejected.length,
              })}</p>
              <ul className="transfer-preview">
                {Array.from({ length: last - first }, (_, offset) => {
                  const index = first + offset;
                  const row = index < count ? undefined : draft.rejected[index - count];
                  const rule = index >= words.length && index < count ? shortcuts[index - words.length] : undefined;
                  const label = row?.entry.label ?? (rule ? `${rule[0]} → ${rule[1]}` : words[index]);
                  return <li key={index}><span>{label.slice(0, 400) || t("dictionary.transfer.blank")}</span>
                    <small>{t(row ? `dictionary.transfer.${row.reason}` : "dictionary.transfer.ready")}</small></li>;
                })}
              </ul>
              {total > PAGE_SIZE && <div className="report-actions">
                <button className="btn ghost" type="button" disabled={page === 0} onClick={() => setPage(page - 1)}>{t("dictionary.transfer.previous")}</button>
                <span role="status">{t("dictionary.transfer.range", { first: first + 1, last, total })}</span>
                <button className="btn ghost" type="button" disabled={last === total} onClick={() => setPage(page + 1)}>{t("dictionary.transfer.next")}</button>
              </div>}
              <div className="report-actions">
                <button className="btn" type="button" disabled={!count || busy} onClick={() => void apply()}>
                  {t("dictionary.transfer.apply")}
                </button>
              </div>
            </>
          )}
        </div>
      </dialog>
    </div>
  );
}
