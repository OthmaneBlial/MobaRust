import { useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { AlertTriangle, FileUp, LoaderCircle, X } from "lucide-react";

type ImportedSession = { id: string; name: string; hostname: string; port: number; username?: string | null; folder?: string | null; auth?: { kind: string; keyRef?: string }; jump_host_profiles?: { host: string }[] } & Record<string, unknown>;
type Candidate = { session: ImportedSession; duplicateOf: string | null };
type Preview = { candidates: Candidate[]; skipped: string[]; warnings: string[] };
type Action = "add" | "replace" | "skip";
type ImportReport = { importedCount: number; skipped: string[] };

const MAX_BYTES = 1024 * 1024;

export function MobaXtermImportDialog({ onClose, onImported }: { onClose: () => void; onImported: (message: string) => void }) {
  const [fileName, setFileName] = useState<string | null>(null);
  const [preview, setPreview] = useState<Preview | null>(null);
  const [actions, setActions] = useState<Action[]>([]);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const choose = async (file: File | undefined) => {
    if (!file) return;
    setError(null);
    setPreview(null);
    if (file.size > MAX_BYTES) {
      setError("This file is larger than 1 MiB. Export only the session folders you need from MobaXterm.");
      return;
    }
    setBusy(true);
    try {
      const bytes = Array.from(new Uint8Array(await file.arrayBuffer()));
      const result = await invoke<Preview>("session_preview_mobaxterm", { payload: { bytes } });
      setFileName(file.name);
      setPreview(result);
      setActions(result.candidates.map((candidate) => candidate.duplicateOf ? "skip" : "add"));
    } catch (failure) {
      setError(String(failure));
    } finally {
      setBusy(false);
    }
  };

  const setAllDuplicates = (action: Action) => setActions((current) => current.map((value, index) => preview?.candidates[index].duplicateOf ? action : value));

  const apply = async () => {
    if (!preview) return;
    setBusy(true);
    setError(null);
    try {
      const decisions = preview.candidates.map((candidate, index) => ({ session: candidate.session, action: actions[index], replaceId: actions[index] === "replace" ? candidate.duplicateOf : null }));
      const report = await invoke<ImportReport>("session_apply_import", { decisions });
      const skipped = preview.skipped.length + report.skipped.length;
      onImported(`Imported ${report.importedCount} MobaXterm session${report.importedCount === 1 ? "" : "s"}${skipped ? ` · ${skipped} skipped` : ""}. Check each imported session's notes for keys to copy into ~/.ssh.`);
      onClose();
    } catch (failure) {
      setError(`Import failed; your existing sessions were not changed. ${String(failure)}`);
    } finally {
      setBusy(false);
    }
  };

  const counts = { add: actions.filter((a) => a === "add").length, replace: actions.filter((a) => a === "replace").length, skip: actions.filter((a) => a === "skip").length };
  const duplicates = preview?.candidates.filter((candidate) => candidate.duplicateOf).length ?? 0;

  return <div className="palette-backdrop" role="presentation" onMouseDown={() => { if (!busy) onClose(); }}>
    <section className="import-dialog" role="dialog" aria-modal="true" aria-labelledby="mobaxterm-import-title" onMouseDown={(event) => event.stopPropagation()} onKeyDown={(event) => { if (event.key === "Escape" && !busy) onClose(); }}>
      <header className="import-dialog-header">
        <div><h2 id="mobaxterm-import-title">Import MobaXterm sessions</h2><p>Choose a <code>.mxtsessions</code>, <code>.moba</code> or <code>MobaXterm.ini</code> file, even one exported on Windows. SSH and SFTP bookmarks come with their folders, gateways and key names; passwords are never read.</p></div>
        <button type="button" className="icon-button" aria-label="Close import" onClick={onClose} disabled={busy}><X size={16} /></button>
      </header>
      <label className="import-file-picker">
        <FileUp size={16} />
        <span>{fileName ?? "Choose file…"}</span>
        <input type="file" accept=".mxtsessions,.moba,.ini,.txt" autoFocus onChange={(event) => void choose(event.target.files?.[0])} disabled={busy} />
      </label>
      {busy && <p className="import-status"><LoaderCircle className="spin" size={14} /> Working…</p>}
      {error && <div className="connect-error" role="alert"><span>{error}</span></div>}
      {preview && <>
        {preview.warnings.length > 0 && <ul className="import-warnings">{preview.warnings.map((warning) => <li key={warning}><AlertTriangle size={13} /> {warning}</li>)}</ul>}
        <div className="import-summary">
          <span>{preview.candidates.length} SSH session{preview.candidates.length === 1 ? "" : "s"} found</span>
          {duplicates > 0 && <span>{duplicates} look like existing sessions · <button type="button" className="link-button" onClick={() => setAllDuplicates("skip")}>skip all</button> · <button type="button" className="link-button" onClick={() => setAllDuplicates("replace")}>replace all</button> · <button type="button" className="link-button" onClick={() => setAllDuplicates("add")}>keep both</button></span>}
        </div>
        {preview.candidates.length > 0 && <div className="import-table-wrap"><table className="import-table">
          <thead><tr><th>Folder</th><th>Name</th><th>Target</th><th>Action</th></tr></thead>
          <tbody>{preview.candidates.map((candidate, index) => <tr key={`${candidate.session.folder}/${candidate.session.name}`}>
            <td>{candidate.session.folder}</td>
            <td>{candidate.session.name}{candidate.duplicateOf && <small className="import-duplicate">duplicate</small>}</td>
            <td><code>{candidate.session.username ? `${candidate.session.username}@` : ""}{candidate.session.hostname}{candidate.session.port !== 22 ? `:${candidate.session.port}` : ""}</code>{!candidate.session.username && <small className="import-duplicate">no user</small>}{candidate.session.jump_host_profiles?.length ? <small className="import-duplicate">via {candidate.session.jump_host_profiles.map((jump) => jump.host).join(" → ")}</small> : null}{candidate.session.auth?.kind === "privateKey" && <small className="import-duplicate">key {candidate.session.auth.keyRef}</small>}</td>
            <td><select aria-label={`Import action for ${candidate.session.name}`} value={actions[index]} onChange={(event) => { const value = event.target.value as Action; setActions((current) => current.map((item, itemIndex) => itemIndex === index ? value : item)); }}>
              <option value="add">{candidate.duplicateOf ? "Keep both" : "Add"}</option>
              {candidate.duplicateOf && <option value="replace">Replace existing</option>}
              <option value="skip">Skip</option>
            </select></td>
          </tr>)}</tbody>
        </table></div>}
        {preview.skipped.length > 0 && <details className="import-skipped"><summary>{preview.skipped.length} bookmark{preview.skipped.length === 1 ? "" : "s"} not imported</summary><ul>{preview.skipped.slice(0, 100).map((item, index) => <li key={`${index}:${item}`}>{item}</li>)}</ul></details>}
      </>}
      <footer className="import-dialog-footer">
        <button type="button" className="outline-button" onClick={onClose} disabled={busy}>Cancel</button>
        <button type="button" className="primary-button" onClick={() => void apply()} disabled={busy || !preview || counts.add + counts.replace === 0}>Import {counts.add + counts.replace > 0 ? `${counts.add + counts.replace} session${counts.add + counts.replace === 1 ? "" : "s"}` : ""}</button>
      </footer>
    </section>
  </div>;
}
