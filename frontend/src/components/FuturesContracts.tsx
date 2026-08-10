import { useState } from "react";
import {
  getFuturesContracts, putFuturesContract, getCtd, uploadCtd,
  ApiError, type FuturesContract, type CtdRecord, type Category,
} from "../api";
import { useFetch } from "../hooks";
import { CATEGORY_LABELS as LABELS, num } from "../fmt";

const CATEGORIES: Category[] = ["equity", "interest_rate", "fx", "credit", "commodity", "other"];

type Draft = Partial<Omit<FuturesContract, "contract_root" | "confirmed">>;

type NewSpec = { contract_root: string } & Omit<FuturesContract, "contract_root" | "confirmed">;
const BLANK_SPEC: NewSpec = {
  contract_root: "", label: "", category: "other", point_value: null,
  currency: "EUR", curve: null, price_convention: "decimal", otc: false,
};

export default function FuturesContracts() {
  const contracts = useFetch(() => getFuturesContracts(), []);
  const ctd = useFetch(() => getCtd(), []);
  const [drafts, setDrafts] = useState<Record<string, Draft>>({});
  const [msg, setMsg] = useState<string | null>(null);
  const [savingRoot, setSavingRoot] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [uploadMsg, setUploadMsg] = useState<string | null>(null);
  const [uploadErr, setUploadErr] = useState<{ msg: string; rows?: { sheet: string; row: number; message: string }[] } | null>(null);
  const [adding, setAdding] = useState<NewSpec | null>(null);

  const rows = contracts.data ?? [];
  const unconfirmedCount = rows.filter((r) => !r.confirmed).length;

  const draftFor = (root: string) => drafts[root] ?? {};
  const setDraft = (root: string, patch: Draft) =>
    setDrafts((d) => ({ ...d, [root]: { ...draftFor(root), ...patch } }));
  const clearDraft = (root: string) =>
    setDrafts((prev) => { const rest = { ...prev }; delete rest[root]; return rest; });

  function effective(r: FuturesContract): Omit<FuturesContract, "contract_root" | "confirmed"> {
    const d = draftFor(r.contract_root);
    return {
      label: d.label !== undefined ? d.label : r.label,
      category: d.category !== undefined ? d.category : r.category,
      point_value: d.point_value !== undefined ? d.point_value : r.point_value,
      currency: d.currency !== undefined ? d.currency : r.currency,
      curve: d.curve !== undefined ? d.curve : r.curve,
      price_convention: d.price_convention !== undefined ? d.price_convention : r.price_convention,
      otc: d.otc !== undefined ? d.otc : r.otc,
    };
  }

  // confirmedOverride is only ever passed by the explicit Confirm/Unconfirm actions below —
  // a plain field-edit Save always keeps the row's existing confirmed value untouched.
  async function save(r: FuturesContract, confirmedOverride?: boolean) {
    setMsg(null);
    setSavingRoot(r.contract_root);
    try {
      const body = { ...effective(r), confirmed: confirmedOverride ?? r.confirmed };
      await putFuturesContract(r.contract_root, body);
      clearDraft(r.contract_root);
      setMsg(`Saved ${r.contract_root}.`);
      contracts.reload();
    } catch (e) {
      const ae = e as ApiError;
      setMsg(`Error: ${ae.detail ?? ae.message}`);
    } finally {
      setSavingRoot((cur) => (cur === r.contract_root ? null : cur));
    }
  }

  // Escape hatch, not the main road. Specs are normally seeded from the NAV
  // Recap on import, with the point value derived from the workbook's own
  // identity and cross-checked — hand-typing one discards that check. It exists
  // for the root the importer legitimately could not derive: a futures row with
  // no ticker, an unparseable root, or a row too incomplete to imply a point
  // value. New rows land unconfirmed, like seeded ones.
  async function saveNew(s: NewSpec) {
    const root = s.contract_root.trim();
    if (!root) { setMsg("Error: a contract root is required."); return; }
    if (rows.some((r) => r.contract_root === root)) {
      setMsg(`Error: ${root} already exists — edit it in the table instead.`);
      return;
    }
    setMsg(null);
    setSavingRoot(root);
    try {
      const { contract_root: _root, ...body } = s;
      await putFuturesContract(root, { ...body, label: body.label.trim() || root, confirmed: false });
      setAdding(null);
      setMsg(`Added ${root}. Check its point value, then confirm it.`);
      contracts.reload();
    } catch (e) {
      const ae = e as ApiError;
      setMsg(`Error: ${ae.detail ?? ae.message}`);
    } finally {
      setSavingRoot((cur) => (cur === root ? null : cur));
    }
  }

  function unconfirm(r: FuturesContract) {
    const ok = window.confirm(
      `Un-confirm ${r.contract_root}? It goes back to being treated as an unverified spec — if its ` +
      `category isn't interest_rate, this pulls it back into the rates DV01 section until someone ` +
      `confirms it again.`,
    );
    if (ok) void save(r, false);
  }

  async function doUploadCtd(f: File) {
    setBusy(true);
    setUploadMsg(null);
    setUploadErr(null);
    try {
      const out = await uploadCtd(f);
      setUploadMsg(`${out.rows} row(s) stored for ${out.nav_date}${out.replaced ? " (replaced)" : ""}.`);
      ctd.reload();
    } catch (e) {
      const ae = e as ApiError;
      setUploadErr({ msg: ae.detail ?? ae.message, rows: ae.rows });
    } finally {
      setBusy(false);
    }
  }

  return (
    <div className="card">
      <h3>Futures contracts</h3>
      {contracts.error && <p className="neg">{contracts.error}</p>}
      {unconfirmedCount > 0 && (
        <p className="warn-badge">
          {unconfirmedCount} contract spec(s) seeded from the workbook still need confirming. The category
          was guessed and the point value derived from the file — check both before confirming.
        </p>
      )}
      {msg && <p className="kpi-sub">{msg}</p>}

      {contracts.data !== null && rows.length === 0 && (
        <p className="kpi-sub">
          No contract specs yet. They are seeded automatically from the NAV Recap when you upload it
          above — one per contract root held, with the point value derived from the file. If this list is
          empty while the fund holds futures, re-upload the same NAV Recap workbook: re-uploading a file
          already on record seeds any missing specs without re-importing anything else.
        </p>
      )}

      <table className="tbl">
        <thead>
          <tr>
            <th>Root</th><th>Label</th><th>Category</th><th>Point value</th>
            <th>Ccy</th><th>Curve</th><th>Price convention</th><th>OTC</th><th>Status</th><th></th>
          </tr>
        </thead>
        <tbody>
          {rows.map((r) => {
            const d = draftFor(r.contract_root);
            const dirty = Object.keys(d).length > 0;
            return (
              <tr key={r.contract_root}>
                <td>{r.contract_root}</td>
                <td>
                  <input
                    value={d.label !== undefined ? d.label : r.label}
                    onChange={(e) => setDraft(r.contract_root, { label: e.target.value })}
                  />
                </td>
                <td>
                  <select
                    value={d.category !== undefined ? d.category : r.category}
                    onChange={(e) => setDraft(r.contract_root, { category: e.target.value as Category })}
                  >
                    {CATEGORIES.map((c) => <option key={c} value={c}>{LABELS[c]}</option>)}
                  </select>
                </td>
                <td>
                  <input
                    type="number" step="any"
                    value={(d.point_value !== undefined ? d.point_value : r.point_value) ?? ""}
                    onChange={(e) => setDraft(r.contract_root, { point_value: e.target.value === "" ? null : Number(e.target.value) })}
                  />
                </td>
                <td>
                  <input
                    value={d.currency !== undefined ? d.currency : r.currency}
                    onChange={(e) => setDraft(r.contract_root, { currency: e.target.value })}
                  />
                </td>
                <td>
                  <input
                    value={(d.curve !== undefined ? d.curve : r.curve) ?? ""}
                    onChange={(e) => setDraft(r.contract_root, { curve: e.target.value === "" ? null : e.target.value })}
                  />
                </td>
                <td>
                  <select
                    value={d.price_convention !== undefined ? d.price_convention : r.price_convention}
                    onChange={(e) => setDraft(r.contract_root, { price_convention: e.target.value as "decimal" | "th32" })}
                  >
                    <option value="decimal">decimal</option>
                    <option value="th32">th32 (32nds)</option>
                  </select>
                </td>
                <td>
                  <input
                    type="checkbox"
                    checked={effective(r).otc}
                    onChange={(e) => setDraft(r.contract_root, { otc: e.target.checked })}
                    title="EMIR: tick if this contract is OTC (executed on a non-equivalent venue or bilaterally). Only OTC notional counts toward the clearing thresholds."
                  />
                </td>
                <td>
                  {r.confirmed ? <span className="pos">confirmed</span> : <span className="warn-badge">unconfirmed</span>}
                </td>
                <td>
                  {(() => {
                    const rowBusy = savingRoot === r.contract_root;
                    return (
                      <>
                        <button disabled={!dirty || rowBusy} onClick={() => void save(r)}>Save</button>
                        {r.confirmed ? (
                          <button disabled={rowBusy} onClick={() => unconfirm(r)}>Unconfirm</button>
                        ) : (
                          <button disabled={rowBusy} onClick={() => void save(r, true)}>Confirm</button>
                        )}
                      </>
                    );
                  })()}
                </td>
              </tr>
            );
          })}
        </tbody>
      </table>

      {adding === null ? (
        <button onClick={() => setAdding({ ...BLANK_SPEC })}>Add a contract root manually</button>
      ) : (
        <div>
          <p className="kpi-sub">
            Only for a root the importer could not derive from the workbook — normally you should
            re-upload the NAV Recap instead and let it seed the spec, so the point value stays
            cross-checked against the file.
          </p>
          <table className="tbl">
            <thead>
              <tr>
                <th>Root</th><th>Label</th><th>Category</th><th>Point value</th>
                <th>Ccy</th><th>Curve</th><th>Price convention</th><th>OTC</th><th></th>
              </tr>
            </thead>
            <tbody>
              <tr>
                <td>
                  <input
                    placeholder="e.g. RX" value={adding.contract_root}
                    onChange={(e) => setAdding({ ...adding, contract_root: e.target.value })}
                  />
                </td>
                <td>
                  <input
                    value={adding.label}
                    onChange={(e) => setAdding({ ...adding, label: e.target.value })}
                  />
                </td>
                <td>
                  <select
                    value={adding.category}
                    onChange={(e) => setAdding({ ...adding, category: e.target.value as Category })}
                  >
                    {CATEGORIES.map((c) => <option key={c} value={c}>{LABELS[c]}</option>)}
                  </select>
                </td>
                <td>
                  <input
                    type="number" step="any" value={adding.point_value ?? ""}
                    onChange={(e) => setAdding({ ...adding, point_value: e.target.value === "" ? null : Number(e.target.value) })}
                  />
                </td>
                <td>
                  <input
                    value={adding.currency}
                    onChange={(e) => setAdding({ ...adding, currency: e.target.value })}
                  />
                </td>
                <td>
                  <input
                    value={adding.curve ?? ""}
                    onChange={(e) => setAdding({ ...adding, curve: e.target.value === "" ? null : e.target.value })}
                  />
                </td>
                <td>
                  <select
                    value={adding.price_convention}
                    onChange={(e) => setAdding({ ...adding, price_convention: e.target.value as "decimal" | "th32" })}
                  >
                    <option value="decimal">decimal</option>
                    <option value="th32">th32 (32nds)</option>
                  </select>
                </td>
                <td>
                  <input
                    type="checkbox"
                    checked={adding.otc}
                    onChange={(e) => setAdding({ ...adding, otc: e.target.checked })}
                    title="EMIR: tick if this contract is OTC (executed on a non-equivalent venue or bilaterally). Only OTC notional counts toward the clearing thresholds."
                  />
                </td>
                <td>
                  <button
                    disabled={savingRoot === adding.contract_root.trim()}
                    onClick={() => void saveNew(adding)}
                  >Add</button>
                  <button onClick={() => { setAdding(null); setMsg(null); }}>Cancel</button>
                </td>
              </tr>
            </tbody>
          </table>
        </div>
      )}

      <h4>Weekly CTD analytics</h4>
      <p className="kpi-sub">
        One row per bond future, with columns nav_date, ticker, ctd_isin, ctd_mod_duration, ctd_clean_price,
        ctd_accrued, conversion_factor. Re-uploading a file for the same NAV date replaces that date's rows.
      </p>
      <input
        type="file" accept=".csv,.xlsx" disabled={busy}
        onChange={(e) => {
          const f = e.target.files?.[0];
          if (f) void doUploadCtd(f);
          e.target.value = "";
        }}
      />
      {uploadMsg && <p className="pos">{uploadMsg}</p>}
      {uploadErr && (
        <div className="neg">
          <p>Upload failed: {uploadErr.msg}</p>
          {uploadErr.rows && (
            <table className="tbl"><tbody>
              {uploadErr.rows.slice(0, 20).map((r, i) => (
                <tr key={i}><td>{r.sheet}</td><td>row {r.row}</td><td>{r.message}</td></tr>
              ))}
            </tbody></table>
          )}
        </div>
      )}
      {ctd.error && <p className="neg">{ctd.error}</p>}
      {(ctd.data ?? []).length > 0 && (
        <table className="tbl">
          <thead>
            <tr><th>Ticker</th><th>CTD ISIN</th><th>Mod. duration</th><th>Clean</th><th>Accrued</th><th>Conv. factor</th></tr>
          </thead>
          <tbody>
            {(ctd.data as CtdRecord[]).map((r) => (
              <tr key={r.ticker}>
                <td>{r.ticker}</td>
                <td>{r.ctd_isin}</td>
                <td>{num(r.ctd_mod_duration)}</td>
                <td>{num(r.ctd_clean_price)}</td>
                <td>{num(r.ctd_accrued)}</td>
                <td>{num(r.conversion_factor)}</td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
    </div>
  );
}
