import { useState } from "react";
import {
  advRequestUrl, bloombergRequestUrl, getAdvDue, uploadBloomberg, ApiError, type BloombergUpload,
} from "../api";
import { useFetch } from "../hooks";

export default function BloombergPanel() {
  const [result, setResult] = useState<BloombergUpload | null>(null);
  const [err, setErr] = useState<ApiError | null>(null);
  const [busy, setBusy] = useState(false);
  const [all, setAll] = useState(false);
  const due = useFetch(() => getAdvDue(), []);

  async function onFile(e: React.ChangeEvent<HTMLInputElement>) {
    const f = e.target.files?.[0];
    if (!f) return;
    setBusy(true);
    setErr(null);
    setResult(null);
    try {
      setResult(await uploadBloomberg(f));
      due.reload();
    } catch (x) {
      setErr(x as ApiError);
    } finally {
      setBusy(false);
      e.target.value = "";
    }
  }

  return (
    <div className="card">
      <h3>Bloomberg classification</h3>
      <p className="kpi-sub">Shared across all portfolios.</p>
      <p className="kpi-sub">
        Export the request workbook, open it in Excel on a machine with a logged-in Bloomberg
        Terminal add-in so the BDP/BDH formulas resolve to values, save it, then upload the
        resolved file back here.
      </p>
      <div className="controls">
        <a href={bloombergRequestUrl} download>Export Bloomberg request</a>
        <input type="file" accept=".xlsx" disabled={busy} onChange={(e) => void onFile(e)} />
        {busy && <span className="kpi-sub">Uploading…</span>}
      </div>

      <div className="controls">
        <a href={all ? `${advRequestUrl}?all=true` : advRequestUrl} download>
          Export ADV request{due.data != null ? ` (${due.data.due} of ${due.data.held} due)` : ""}
        </a>
        <label><input type="checkbox" checked={all} onChange={(e) => setAll(e.target.checked)} /> full rebuild</label>
      </div>
      <p className="kpi-sub">
        Formulas resolve only when you open the file in Excel on a machine with a
        logged-in Bloomberg Terminal. Nothing here queries Bloomberg on its own.
      </p>

      {result && (
        <div>
          <p className="pos">
            {result.classified} instrument(s) classified, {result.fx_rows} FX rate(s) stored,{" "}
            {result.adv_rows} ADV volume(s) stored.
          </p>
          {result.skipped.length === 0 && result.fx_check.length === 0 && result.fx_check_skipped.length === 0 && (
            <p className="kpi-sub">No skipped cells and no FX cross-check drift.</p>
          )}
          {result.fx_check_skipped.length > 0 && (
            <p className="warn-badge">
              FX cross-check not run for {result.fx_check_skipped.length} portfolio(s) you cannot
              view: {result.fx_check_skipped.map((s) => s.portfolio_name).join(", ")}. Absence of
              drift above does not cover these.
            </p>
          )}
          {result.skipped.length > 0 && (
            <>
              <p className="warn-badge">
                {result.skipped.length} cell(s) did not resolve and were not stored:
              </p>
              <table className="tbl"><tbody>
                {result.skipped.slice(0, 20).map((s, i) => (
                  <tr key={i}><td>{s.sheet}</td><td>row {s.row}</td><td>{s.message}</td></tr>
                ))}
              </tbody></table>
            </>
          )}
          {result.fx_check.length > 0 && (
            <>
              <p className="neg">
                FX cross-check failed — these rates disagree with the NAV Recap&apos;s own Change
                column, which usually means the quote is inverted:
              </p>
              <table className="tbl">
                <thead>
                  <tr><th>Currency</th><th>Date</th><th>Workbook</th><th>Bloomberg</th><th>Drift</th></tr>
                </thead>
                <tbody>
                  {result.fx_check.map((c, i) => (
                    <tr key={i}>
                      <td>{c.currency}</td>
                      <td>{c.date}</td>
                      <td>{c.workbook.toFixed(4)}</td>
                      <td>{c.bloomberg.toFixed(4)}</td>
                      <td>{(c.drift * 100).toFixed(1)}%</td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </>
          )}
        </div>
      )}

      {err && (
        <div className="neg">
          <p>Upload failed: {err.detail ?? err.message}</p>
          {err.rows && (
            <table className="tbl"><tbody>
              {err.rows.slice(0, 20).map((r, i) => (
                <tr key={i}><td>{r.sheet}</td><td>row {r.row}</td><td>{r.message}</td></tr>
              ))}
            </tbody></table>
          )}
        </div>
      )}
    </div>
  );
}
