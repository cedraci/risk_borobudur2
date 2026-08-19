import { useState } from "react";
import {
  ADMIN_ACTIONS, ADMIN_DOMAINS, ApiError, addGrant, domainLabel, getGrants, removeGrant,
  type Portfolio,
} from "../api";
import { useFetch } from "../hooks";
import Unavailable from "./Unavailable";

/** The six-domain by four-action grant matrix for one user at one scope — a
 * named portfolio, or "all portfolios" when the selected scope is null.
 * Checking `export`/`import`/`configure` implies `view` for that same domain
 * and scope: `db::auth::model::GrantSet::from_grants`
 * (`crates/db/src/auth/model.rs`) expands exactly that server-side, so `view`
 * is shown auto-checked and disabled here rather than left as a second click
 * that the server would just no-op. */
export default function GrantEditor({ userId, portfolios }: { userId: number; portfolios: Portfolio[] }) {
  const [scope, setScope] = useState<number | null>(null);
  const grants = useFetch(() => getGrants(userId), [userId]);
  const [pending, setPending] = useState<string | null>(null);
  const [msg, setMsg] = useState<string | null>(null);

  if (grants.forbidden) return <Unavailable reason={grants.forbidden} />;
  if (grants.error) return <p className="neg">{grants.error}</p>;
  if (!grants.data) return <p>Loading…</p>;

  const scoped = grants.data.filter((g) => g.portfolio === scope);
  const has = (domain: string, action: string) => scoped.some((g) => g.domain === domain && g.action === action);

  async function toggle(domain: string, action: string, checked: boolean) {
    const key = `${domain}:${action}`;
    setPending(key);
    setMsg(null);
    try {
      if (checked) await addGrant(userId, { domain, action, portfolio: scope });
      else await removeGrant(userId, { domain, action, portfolio: scope });
      grants.reload();
    } catch (e) {
      const ae = e as ApiError;
      setMsg(`Error: ${ae.detail ?? ae.message}`);
    } finally {
      setPending(null);
    }
  }

  return (
    <div>
      <div className="controls">
        <label>Scope{" "}
          <select
            value={scope === null ? "" : scope}
            onChange={(e) => setScope(e.target.value === "" ? null : Number(e.target.value))}
          >
            <option value="">All portfolios</option>
            {portfolios.map((p) => <option key={p.id} value={p.id}>{p.name}</option>)}
          </select>
        </label>
      </div>
      {msg && <p className="neg">{msg}</p>}
      <table className="tbl">
        <thead>
          <tr>
            <th>Domain</th>
            {ADMIN_ACTIONS.map((a) => <th key={a}>{a}</th>)}
          </tr>
        </thead>
        <tbody>
          {ADMIN_DOMAINS.map((d) => {
            const impliedView = ADMIN_ACTIONS.some((a) => a !== "view" && has(d, a));
            return (
              <tr key={d}>
                <td>{domainLabel(d)}</td>
                {ADMIN_ACTIONS.map((a) => {
                  const checked = a === "view" ? (impliedView || has(d, a)) : has(d, a);
                  const key = `${d}:${a}`;
                  const disabled = (a === "view" && impliedView) || pending === key;
                  return (
                    <td key={a}>
                      <input
                        type="checkbox"
                        checked={checked}
                        disabled={disabled}
                        onChange={(e) => void toggle(d, a, e.target.checked)}
                      />
                    </td>
                  );
                })}
              </tr>
            );
          })}
        </tbody>
      </table>
    </div>
  );
}
