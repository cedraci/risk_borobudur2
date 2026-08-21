import { useState } from "react";
import {
  acknowledgeBreach, ApiError, breachExportUrl, getBreaches, getLimitRuns, rerunLimitChecks, resolveBreach,
  type BreachEpisode, type CheckResult, type LimitRun,
} from "../api";
import Unavailable from "../components/Unavailable";
import { eur, pct } from "../fmt";
import { useFetch } from "../hooks";
import { usePortfolio } from "../PortfolioContext";

/** One label + colour set per state/classification value, rendered as the
 * same bordered pill for all six — so "Open" (urgent, unaddressed) doesn't
 * read as a bare red word next to "Acknowledged"'s boxed amber pill while
 * "Resolved" is a bare green word. `active` gets its own colour rather than
 * sharing green with `resolved`/`passive`: it is the manager's own decision,
 * not a cleared item, and painting it pass-green invited exactly that
 * misreading on a scan. */
const CHIP: Record<string, { label: string; bg: string; fg: string; border: string }> = {
  open: { label: "Open", bg: "#fdeaea", fg: "#c62828", border: "#f0b4b4" },
  acknowledged: { label: "Acknowledged", bg: "#fff4e0", fg: "#925b06", border: "#f0c36d" },
  resolved: { label: "Resolved", bg: "#e8f5ec", fg: "#0a7d33", border: "#a9d9b8" },
  unclassified: { label: "Unclassified", bg: "#fff4e0", fg: "#925b06", border: "#f0c36d" },
  active: { label: "Active", bg: "#e8f0fb", fg: "#1d4ea3", border: "#b7cff0" },
  passive: { label: "Passive", bg: "#e8f5ec", fg: "#0a7d33", border: "#a9d9b8" },
};

function Chip({ kind }: { kind: keyof typeof CHIP }) {
  // `kind` is wire data, not a closed set: a state or classification added
  // server-side before the frontend ships would otherwise throw on `c.bg` and
  // unmount the whole page — a blank screen instead of one odd-looking chip.
  // Same defensive lookup as `DerivativesPage`'s `VERDICT_LABEL[v] ?? v`.
  const c = CHIP[kind] ?? { label: kind, bg: "#eceff1", fg: "#455a64", border: "#cfd8dc" };
  return (
    <span style={{
      display: "inline-block", background: c.bg, color: c.fg, border: `1px solid ${c.border}`,
      borderRadius: 4, padding: "2px 8px", fontSize: 12, marginLeft: 6,
    }}>
      {c.label}
    </span>
  );
}

function daysBetween(from: string, to: string): number {
  const ms = new Date(`${to}T00:00:00Z`).getTime() - new Date(`${from}T00:00:00Z`).getTime();
  return Math.max(0, Math.round(ms / 86_400_000));
}

/** The check's own scope/limit come from the newest run's matching result,
 * not the episode row — an episode has no copy of the limit it breached,
 * only what it observed. */
function latestResultFor(runs: LimitRun[], checkKey: string): CheckResult | undefined {
  return runs[0]?.results.find((r) => r.check_key === checkKey);
}

/** EMIR checks (`emir_*`) record a euro notional against a euro threshold
 * (`handlers/breaches.rs`'s `threshold_eur`/`avg_otc_eur`); every other check
 * family (concentration, liquidity, VaR) records a ratio to NAV or to the
 * configured limit fraction. Formatting an EMIR episode with `pct` would
 * print a nine-figure euro amount as a percentage — exactly the class of
 * episode a regulator asks about first. */
function valueFmt(checkKey: string): (x: number | null | undefined) => string {
  return checkKey.startsWith("emir_") ? eur : pct;
}

/** Sorted `open` first, then everything else by `opened_nav_date` ascending —
 * the oldest unaddressed episode leads, and a resolved episode never buries
 * a live one further down the list. */
function sortEpisodes(episodes: BreachEpisode[]): BreachEpisode[] {
  return [...episodes].sort((a, b) => {
    const rank = (e: BreachEpisode) => (e.state === "open" ? 0 : 1);
    const r = rank(a) - rank(b);
    return r !== 0 ? r : a.opened_nav_date.localeCompare(b.opened_nav_date);
  });
}

function AcknowledgeForm({ pid, bid, onDone }: { pid: number; bid: number; onDone: () => void }) {
  const [classification, setClassification] = useState<"active" | "passive" | "">("");
  const [note, setNote] = useState("");
  const [deadline, setDeadline] = useState("");
  const [busy, setBusy] = useState(false);
  // Kept apart from `serverErr` so the user can tell "you left the form
  // incomplete" from "the server refused this" (e.g. someone else already
  // acted) — the same red text for both reads as one generic failure.
  const [formErr, setFormErr] = useState<string | null>(null);
  const [serverErr, setServerErr] = useState<string | null>(null);
  // A 403 is a denial, not a finding: it must never take the red used for a
  // breach. `Unavailable`'s own contract names this case — see its doc
  // comment — so it is tracked apart from `serverErr` rather than styled
  // differently at the call site.
  const [denied, setDenied] = useState<string | null>(null);

  async function submit() {
    setServerErr(null);
    setDenied(null);
    if (classification === "" || note.trim() === "") {
      setFormErr("Choose a classification and add a note before acknowledging.");
      return;
    }
    setFormErr(null);
    setBusy(true);
    try {
      await acknowledgeBreach(pid, bid, {
        classification, note: note.trim(),
        ...(deadline ? { deadline_date: deadline } : {}),
      });
      onDone();
    } catch (e) {
      const ae = e as ApiError;
      if (ae.status === 403) setDenied(ae.detail ?? ae.message);
      else setServerErr(ae.detail ?? ae.message);
    } finally {
      setBusy(false);
    }
  }

  return (
    <div className="controls" style={{ flexDirection: "column", alignItems: "flex-start", gap: 6 }}>
      <label>
        <input
          type="radio" name={`classification-${bid}`} checked={classification === "active"}
          onChange={() => setClassification("active")}
        /> Active — a decision the manager made
      </label>
      <label>
        <input
          type="radio" name={`classification-${bid}`} checked={classification === "passive"}
          onChange={() => setClassification("passive")}
        /> Passive — market movement, not a trade
      </label>
      <textarea
        placeholder="Note (required)" value={note} rows={2}
        onChange={(e) => setNote(e.target.value)} style={{ width: "100%" }}
      />
      <label>Deadline (optional):{" "}
        <input type="date" value={deadline} onChange={(e) => setDeadline(e.target.value)} />
      </label>
      {formErr && <p className="warn-badge">{formErr}</p>}
      {denied && <Unavailable reason={denied} />}
      {serverErr && <p className="neg">{serverErr}</p>}
      <button type="button" disabled={busy} onClick={() => void submit()}>Acknowledge</button>
    </div>
  );
}

function ResolveForm({ pid, bid, onDone }: { pid: number; bid: number; onDone: () => void }) {
  const [note, setNote] = useState("");
  const [busy, setBusy] = useState(false);
  const [formErr, setFormErr] = useState<string | null>(null);
  const [serverErr, setServerErr] = useState<string | null>(null);
  const [denied, setDenied] = useState<string | null>(null);

  async function submit() {
    setServerErr(null);
    setDenied(null);
    if (note.trim() === "") {
      setFormErr("A note is required to resolve.");
      return;
    }
    setFormErr(null);
    setBusy(true);
    try {
      await resolveBreach(pid, bid, note.trim());
      onDone();
    } catch (e) {
      const ae = e as ApiError;
      if (ae.status === 403) setDenied(ae.detail ?? ae.message);
      else setServerErr(ae.detail ?? ae.message);
    } finally {
      setBusy(false);
    }
  }

  return (
    <div className="controls" style={{ flexDirection: "column", alignItems: "flex-start", gap: 6 }}>
      <textarea
        placeholder="Resolution note (required)" value={note} rows={2}
        onChange={(e) => setNote(e.target.value)} style={{ width: "100%" }}
      />
      {formErr && <p className="warn-badge">{formErr}</p>}
      {denied && <Unavailable reason={denied} />}
      {serverErr && <p className="neg">{serverErr}</p>}
      <button type="button" disabled={busy} onClick={() => void submit()}>Resolve</button>
    </div>
  );
}

function EpisodeCard({
  pid, ep, runs, runsUnavailable, onChanged,
}: { pid: number; ep: BreachEpisode; runs: LimitRun[]; runsUnavailable?: string; onChanged: () => void }) {
  const result = latestResultFor(runs, ep.check_key);
  const fmt = valueFmt(ep.check_key);
  const today = new Date().toISOString().slice(0, 10);
  const days = daysBetween(ep.opened_nav_date, ep.closed_nav_date ?? today);

  return (
    <div className="card">
      <h4>
        {result?.scope_label ?? ep.check_key} — {ep.subject}
        <Chip kind={ep.state} />
        <Chip kind={ep.classification} />
      </h4>
      {/* Without the run history, the check's own scope label and limit are
          unknown — the card still shows what it can (the raw check key, the
          episode's own observed/peak figures) but says why the rest is
          missing, rather than leaving a bare "vs limit –" unexplained. */}
      {runsUnavailable && (
        <p className="unavailable">Check details (scope label, limit) unavailable — {runsUnavailable}.</p>
      )}
      <p className="kpi-sub">
        Open since {ep.opened_nav_date} ({days} day{days === 1 ? "" : "s"}) ·{" "}
        {fmt(ep.opened_value)} &rarr; peak {fmt(ep.peak_value)} vs limit {fmt(result?.limit_value ?? null)}
      </p>
      {/* The server closes an episode as soon as the numbers clear, but that
          is not the same as a human signing off on it — this line is the one
          place the page says "the data moved, a person still has to look". */}
      {ep.closed_nav_date && ep.state !== "resolved" && (
        <p className="warn-badge">cleared on the data since {ep.closed_nav_date} — awaiting sign-off</p>
      )}
      {/* Never a decision — labelled "Proposed:" and kept in one text run
          (no nested element) so it stays a single, unambiguous sentence.
          `proposed_classification` is `null` in three real cases (first
          snapshot, no holdings, or a quantity missing at one of the two
          snapshots — see `analytics::breach::propose`), each still carrying
          a `proposal_reason`. Falling through to "Passive" there would
          manufacture a guess the engine explicitly declined to make, right
          next to the sentence explaining why it couldn't — worse than
          presenting a real proposal as a decision, so it is called out on
          its own rather than folded into the active/passive wording. */}
      {ep.proposal_reason && (
        <p className="kpi-sub">
          {ep.proposed_classification === null
            ? `No proposal — ${ep.proposal_reason}`
            : `Proposed: ${ep.proposed_classification === "active" ? "Active" : "Passive"} — ${ep.proposal_reason}`}
        </p>
      )}
      {ep.acknowledged_at && (
        <p className="kpi-sub">
          Acknowledged {new Date(ep.acknowledged_at).toLocaleString("fr-FR")} by{" "}
          {ep.acknowledged_by_label ?? "unknown"}
          {ep.acknowledgement_note ? ` — "${ep.acknowledgement_note}"` : ""}
          {ep.deadline_date ? `, deadline ${ep.deadline_date}` : ""}
        </p>
      )}
      {ep.resolved_at && (
        <p className="kpi-sub">
          Resolved {new Date(ep.resolved_at).toLocaleString("fr-FR")} by{" "}
          {ep.resolved_by_label ?? "unknown"}
          {ep.resolution_note ? ` — "${ep.resolution_note}"` : ""}
        </p>
      )}
      {ep.state === "open" && <AcknowledgeForm pid={pid} bid={ep.id} onDone={onChanged} />}
      {ep.state === "acknowledged" && <ResolveForm pid={pid} bid={ep.id} onDone={onChanged} />}
    </div>
  );
}

function RunHistory({ runs }: { runs: LimitRun[] }) {
  const checkKeys = Array.from(new Set(runs.flatMap((r) => r.results.map((res) => res.check_key))));
  const labelFor = (key: string) =>
    runs.flatMap((r) => r.results).find((res) => res.check_key === key)?.scope_label ?? key;

  if (runs.length === 0) return <p className="kpi-sub">No runs recorded yet.</p>;

  return (
    // 52 runs (the default `getLimitRuns` page size) means 52 columns —
    // wide enough to break the page layout without its own scroll container.
    <div style={{ overflowX: "auto" }}>
      <table className="tbl">
        <thead>
          <tr>
            <th>Check</th>
            {runs.map((r) => (
              <th
                key={r.id}
                // Each note is keyed by the check it belongs to
                // (`input_notes` in `handlers/breaches.rs`) — dropping the
                // key here would leave "no register loaded" without saying
                // which check went unevaluated.
                title={r.inputs_complete
                  ? undefined
                  : Object.entries(r.input_notes).map(([k, v]) => `${k}: ${v}`).join("; ")}
              >
                {r.nav_date}{" "}
                {!r.inputs_complete && <span className="warn-badge">incomplete</span>}
              </th>
            ))}
          </tr>
        </thead>
        <tbody>
          {checkKeys.map((key) => (
            <tr key={key}>
              <td>{labelFor(key)}</td>
              {runs.map((r) => {
                const res = r.results.find((x) => x.check_key === key);
                const cls = !res ? "unavailable" : res.status === "ok" ? "pos" : res.status === "watch" ? "warn-badge" : "neg";
                return <td key={r.id}><span className={cls}>{res ? res.status.toUpperCase() : "N/A"}</span></td>;
              })}
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}

export default function BreachesPage() {
  const portfolio = usePortfolio();
  const runs = useFetch(() => getLimitRuns(portfolio.id), [portfolio.id]);
  const breaches = useFetch(() => getBreaches(portfolio.id), [portfolio.id]);
  const [rerunBusy, setRerunBusy] = useState(false);
  const [rerunErr, setRerunErr] = useState<string | null>(null);
  const [rerunDenied, setRerunDenied] = useState<string | null>(null);

  function reloadAll() {
    runs.reload();
    breaches.reload();
  }

  async function doRerun() {
    setRerunBusy(true);
    setRerunErr(null);
    setRerunDenied(null);
    try {
      await rerunLimitChecks(portfolio.id);
      reloadAll();
    } catch (e) {
      const ae = e as ApiError;
      if (ae.status === 403) setRerunDenied(ae.detail ?? ae.message);
      else setRerunErr(ae.detail ?? ae.message);
    } finally {
      setRerunBusy(false);
    }
  }

  const runRows = runs.data?.runs ?? [];
  const episodes = sortEpisodes(breaches.data?.breaches ?? []);

  return (
    <div>
      <h2>Breaches</h2>

      {/* Titled for what it sorts by, not what it holds: every episode is
          listed here (open first, then by opened date), so a resolved one
          still shows up — only its own state chip and the sort position say
          it is no longer live. */}
      <h3>Breach episodes</h3>
      {breaches.forbidden ? (
        <Unavailable reason={breaches.forbidden} />
      ) : breaches.error ? (
        <p className="neg">{breaches.error}</p>
      ) : breaches.data === null ? null : episodes.length === 0 ? (
        <p className="kpi-sub">No breaches in the register.</p>
      ) : (
        episodes.map((ep) => (
          <EpisodeCard
            key={ep.id} pid={portfolio.id} ep={ep} runs={runRows}
            runsUnavailable={runs.forbidden} onChanged={reloadAll}
          />
        ))
      )}

      <h3>Run history</h3>
      <div className="controls">
        <button type="button" disabled={rerunBusy} onClick={() => void doRerun()}>
          {rerunBusy ? "Re-running…" : "Re-run checks now"}
        </button>
        <a href={breachExportUrl(portfolio.id)} download>Export evidence workbook</a>
      </div>
      {rerunDenied && <Unavailable reason={rerunDenied} />}
      {rerunErr && <p className="neg">{rerunErr}</p>}
      {runs.forbidden ? <Unavailable reason={runs.forbidden} /> : runs.error && <p className="neg">{runs.error}</p>}
      {!runs.forbidden && !runs.error && <RunHistory runs={runRows} />}
    </div>
  );
}
