import { useState } from "react";
import {
  acknowledgeBreach, ApiError, breachExportUrl, getBreaches, getLimitRuns, rerunLimitChecks, resolveBreach,
  type BreachEpisode, type CheckResult, type LimitRun,
} from "../api";
import Unavailable from "../components/Unavailable";
import { pct } from "../fmt";
import { useFetch } from "../hooks";
import { usePortfolio } from "../PortfolioContext";

const STATE_LABEL: Record<BreachEpisode["state"], string> = {
  open: "Open", acknowledged: "Acknowledged", resolved: "Resolved",
};
const STATE_CLASS: Record<BreachEpisode["state"], string> = {
  open: "neg", acknowledged: "warn-badge", resolved: "pos",
};
const CLASS_LABEL: Record<BreachEpisode["classification"], string> = {
  unclassified: "Unclassified", active: "Active", passive: "Passive",
};

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
  const [err, setErr] = useState<string | null>(null);

  async function submit() {
    if (classification === "" || note.trim() === "") {
      setErr("A classification and a note are both required to acknowledge.");
      return;
    }
    setBusy(true);
    setErr(null);
    try {
      await acknowledgeBreach(pid, bid, {
        classification, note: note.trim(),
        ...(deadline ? { deadline_date: deadline } : {}),
      });
      onDone();
    } catch (e) {
      const ae = e as ApiError;
      setErr(ae.detail ?? ae.message);
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
      {err && <p className="neg">{err}</p>}
      <button type="button" disabled={busy} onClick={() => void submit()}>Acknowledge</button>
    </div>
  );
}

function ResolveForm({ pid, bid, onDone }: { pid: number; bid: number; onDone: () => void }) {
  const [note, setNote] = useState("");
  const [busy, setBusy] = useState(false);
  const [err, setErr] = useState<string | null>(null);

  async function submit() {
    if (note.trim() === "") {
      setErr("A note is required to resolve.");
      return;
    }
    setBusy(true);
    setErr(null);
    try {
      await resolveBreach(pid, bid, note.trim());
      onDone();
    } catch (e) {
      const ae = e as ApiError;
      setErr(ae.detail ?? ae.message);
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
      {err && <p className="neg">{err}</p>}
      <button type="button" disabled={busy} onClick={() => void submit()}>Resolve</button>
    </div>
  );
}

function EpisodeCard({
  pid, ep, runs, onChanged,
}: { pid: number; ep: BreachEpisode; runs: LimitRun[]; onChanged: () => void }) {
  const result = latestResultFor(runs, ep.check_key);
  const today = new Date().toISOString().slice(0, 10);
  const days = daysBetween(ep.opened_nav_date, ep.closed_nav_date ?? today);

  return (
    <div className="card">
      <h4>
        {result?.scope_label ?? ep.check_key} — {ep.subject}{" "}
        <span className={STATE_CLASS[ep.state]}>{STATE_LABEL[ep.state]}</span>{" "}
        <span className={ep.classification === "unclassified" ? "warn-badge" : "pos"}>
          {CLASS_LABEL[ep.classification]}
        </span>
      </h4>
      <p className="kpi-sub">
        Open since {ep.opened_nav_date} ({days} day{days === 1 ? "" : "s"}) ·{" "}
        {pct(ep.opened_value)} &rarr; peak {pct(ep.peak_value)} vs limit {pct(result?.limit_value ?? null)}
      </p>
      {/* The server closes an episode as soon as the numbers clear, but that
          is not the same as a human signing off on it — this line is the one
          place the page says "the data moved, a person still has to look". */}
      {ep.closed_nav_date && ep.state !== "resolved" && (
        <p className="warn-badge">cleared on the data since {ep.closed_nav_date} — awaiting sign-off</p>
      )}
      {/* Never a decision — labelled "Proposed:" and kept in one text run
          (no nested element) so it stays a single, unambiguous sentence. */}
      {ep.proposal_reason && (
        <p className="kpi-sub">
          Proposed: {ep.proposed_classification === "active" ? "Active" : "Passive"} — {ep.proposal_reason}
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
    <table className="tbl">
      <thead>
        <tr>
          <th>Check</th>
          {runs.map((r) => (
            <th
              key={r.id}
              title={r.inputs_complete ? undefined : Object.values(r.input_notes).join("; ")}
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
  );
}

export default function BreachesPage() {
  const portfolio = usePortfolio();
  const runs = useFetch(() => getLimitRuns(portfolio.id), [portfolio.id]);
  const breaches = useFetch(() => getBreaches(portfolio.id), [portfolio.id]);
  const [rerunBusy, setRerunBusy] = useState(false);
  const [rerunErr, setRerunErr] = useState<string | null>(null);

  function reloadAll() {
    runs.reload();
    breaches.reload();
  }

  async function doRerun() {
    setRerunBusy(true);
    setRerunErr(null);
    try {
      await rerunLimitChecks(portfolio.id);
      reloadAll();
    } catch (e) {
      const ae = e as ApiError;
      setRerunErr(ae.detail ?? ae.message);
    } finally {
      setRerunBusy(false);
    }
  }

  const runRows = runs.data?.runs ?? [];
  const episodes = sortEpisodes(breaches.data?.breaches ?? []);

  return (
    <div>
      <h2>Breaches</h2>

      <h3>Open episodes</h3>
      {breaches.forbidden ? (
        <Unavailable reason={breaches.forbidden} />
      ) : breaches.error ? (
        <p className="neg">{breaches.error}</p>
      ) : episodes.length === 0 ? (
        <p className="kpi-sub">No breaches in the register.</p>
      ) : (
        episodes.map((ep) => (
          <EpisodeCard key={ep.id} pid={portfolio.id} ep={ep} runs={runRows} onChanged={reloadAll} />
        ))
      )}

      <h3>Run history</h3>
      <div className="controls">
        <button type="button" disabled={rerunBusy} onClick={() => void doRerun()}>
          {rerunBusy ? "Re-running…" : "Re-run checks now"}
        </button>
        <a href={breachExportUrl(portfolio.id)} download>Export evidence workbook</a>
      </div>
      {rerunErr && <p className="neg">{rerunErr}</p>}
      {runs.forbidden ? <Unavailable reason={runs.forbidden} /> : runs.error && <p className="neg">{runs.error}</p>}
      {!runs.forbidden && !runs.error && <RunHistory runs={runRows} />}
    </div>
  );
}
