import { getAudit } from "../api";
import { useFetch } from "../hooks";
import Unavailable from "./Unavailable";

/** Newest-first, capped at 200 rows — the server already orders by `at DESC,
 * id DESC` (`crates/db/src/admin.rs::audit_recent`). Read-only: there is no
 * delete endpoint, so no delete control belongs here either. */
export default function AuditLog() {
  const audit = useFetch(() => getAudit(200), []);

  if (audit.forbidden) return <Unavailable reason={audit.forbidden} />;
  if (audit.error) return <p className="neg">{audit.error}</p>;
  if (!audit.data) return <p>Loading…</p>;

  return (
    <table className="tbl">
      <thead>
        <tr><th>Time</th><th>Actor</th><th>Action</th><th>Domain</th><th>Portfolio</th><th>Detail</th></tr>
      </thead>
      <tbody>
        {audit.data.map((r) => (
          <tr key={r.id}>
            <td>{new Date(r.at).toLocaleString("fr-FR")}</td>
            <td>{r.actor_label}</td>
            <td>{r.action}</td>
            <td>{r.domain ?? "—"}</td>
            <td>{r.portfolio_id ?? "—"}</td>
            <td><code>{JSON.stringify(r.detail)}</code></td>
          </tr>
        ))}
      </tbody>
    </table>
  );
}
