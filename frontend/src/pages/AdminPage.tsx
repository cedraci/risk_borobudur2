import { useState } from "react";
import {
  ADMIN_ROLES, ApiError, adminSetDisabled, adminSetPassword, assignRole, createUser, getPortfolios, getUsers,
  type AdminUser, type Portfolio,
} from "../api";
import AuditLog from "../components/AuditLog";
import GrantEditor from "../components/GrantEditor";
import Unavailable from "../components/Unavailable";
import { useFetch } from "../hooks";

/** A cryptographically random password, generated client-side and shown to
 * the caller exactly once. `POST /api/admin/users` and
 * `PUT /api/admin/users/{id}/password` (`crates/server/src/handlers/admin.rs`)
 * both take a client-supplied password rather than generating one
 * server-side — the server has no generated value to hand back, so this is
 * the only place one ever exists. Nothing here persists it: once the
 * "Generated password" card is dismissed (or the page navigates away) it is
 * gone for good, same as the server never storing it in recoverable form. */
function genPassword(length = 20): string {
  const charset = "ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnpqrstuvwxyz23456789!@#$%^&*-_=+";
  const bytes = new Uint32Array(length);
  crypto.getRandomValues(bytes);
  return Array.from(bytes, (b) => charset[b % charset.length]).join("");
}

export default function AdminPage() {
  const users = useFetch(() => getUsers(), []);
  const portfolios = useFetch(() => getPortfolios(), []);
  const [selected, setSelected] = useState<number | null>(null);
  const [shown, setShown] = useState<{ email: string; password: string } | null>(null);
  const [msg, setMsg] = useState<string | null>(null);

  if (users.forbidden) {
    return (
      <div>
        <h2>Administration</h2>
        <Unavailable reason={users.forbidden} />
      </div>
    );
  }

  async function doReset(u: AdminUser) {
    setMsg(null);
    const password = genPassword();
    try {
      await adminSetPassword(u.id, password);
      setShown({ email: u.email, password });
    } catch (e) {
      const ae = e as ApiError;
      setMsg(`Error: ${ae.detail ?? ae.message}`);
    }
  }

  async function doToggleDisabled(u: AdminUser) {
    setMsg(null);
    try {
      await adminSetDisabled(u.id, !u.disabled);
      users.reload();
    } catch (e) {
      const ae = e as ApiError;
      setMsg(`Error: ${ae.detail ?? ae.message}`);
    }
  }

  const selectedUser = users.data?.find((u) => u.id === selected) ?? null;

  return (
    <div>
      <h2>Administration</h2>
      {msg && <p className="neg">{msg}</p>}

      {shown && (
        <div className="card">
          <h3>Generated password</h3>
          <p>
            For <strong>{shown.email}</strong> — shown once here and never again.
            Copy it now and hand it to the user out of band.
          </p>
          <p><code>{shown.password}</code></p>
          <button type="button" onClick={() => setShown(null)}>Dismiss</button>
        </div>
      )}

      <UsersCard
        users={users.data ?? []}
        error={users.error}
        onSelect={setSelected}
        onCreated={(email, password) => { setShown({ email, password }); users.reload(); }}
        onReset={doReset}
        onToggleDisabled={doToggleDisabled}
      />

      {selectedUser && (
        <div className="card">
          <h3>Permissions — {selectedUser.display_name}</h3>
          <GrantEditor userId={selectedUser.id} portfolios={portfolios.data ?? []} />
          <RoleAssign userId={selectedUser.id} portfolios={portfolios.data ?? []} />
        </div>
      )}

      <div className="card">
        <h3>Audit log</h3>
        <AuditLog />
      </div>
    </div>
  );
}

function UsersCard({
  users, error, onSelect, onCreated, onReset, onToggleDisabled,
}: {
  users: AdminUser[]; error: string | null; onSelect: (id: number) => void;
  onCreated: (email: string, password: string) => void;
  onReset: (u: AdminUser) => void; onToggleDisabled: (u: AdminUser) => void;
}) {
  const [email, setEmail] = useState("");
  const [name, setName] = useState("");
  const [isAdmin, setIsAdmin] = useState(false);
  const [busy, setBusy] = useState(false);
  const [createErr, setCreateErr] = useState<string | null>(null);

  async function doCreate(e: React.FormEvent) {
    e.preventDefault();
    setBusy(true);
    setCreateErr(null);
    const password = genPassword();
    try {
      const u = await createUser({
        email: email.trim(), display_name: name.trim(), password, is_administrator: isAdmin,
      });
      setEmail(""); setName(""); setIsAdmin(false);
      onCreated(u.email, password);
    } catch (e) {
      const ae = e as ApiError;
      setCreateErr(ae.detail ?? ae.message);
    } finally {
      setBusy(false);
    }
  }

  return (
    <div className="card">
      <h3>Users</h3>
      {error && <p className="neg">{error}</p>}
      <table className="tbl">
        <thead><tr><th>Name</th><th>Email</th><th>Administrator</th><th>Disabled</th><th></th></tr></thead>
        <tbody>
          {users.map((u) => (
            <tr key={u.id}>
              <td>{u.display_name}</td>
              <td>{u.email}</td>
              <td>{u.is_administrator ? "yes" : "—"}</td>
              <td>{u.disabled ? "yes" : "—"}</td>
              <td>
                <button type="button" onClick={() => onSelect(u.id)}>Permissions</button>
                <button type="button" onClick={() => void onReset(u)}>Reset password</button>
                <button type="button" onClick={() => void onToggleDisabled(u)}>{u.disabled ? "Enable" : "Disable"}</button>
              </td>
            </tr>
          ))}
        </tbody>
      </table>

      <h4>Create user</h4>
      <form className="controls" onSubmit={(e) => void doCreate(e)}>
        <label>Email <input type="email" required value={email} onChange={(e) => setEmail(e.target.value)} /></label>
        <label>Display name <input required value={name} onChange={(e) => setName(e.target.value)} /></label>
        <label><input type="checkbox" checked={isAdmin} onChange={(e) => setIsAdmin(e.target.checked)} /> Administrator</label>
        <button type="submit" disabled={busy || !email.trim() || !name.trim()}>Create</button>
        {createErr && <span className="neg">{createErr}</span>}
      </form>
    </div>
  );
}

function RoleAssign({ userId, portfolios }: { userId: number; portfolios: Portfolio[] }) {
  const [role, setRole] = useState(ADMIN_ROLES[0].value);
  const [scope, setScope] = useState<number | null>(null);
  const [busy, setBusy] = useState(false);
  const [msg, setMsg] = useState<string | null>(null);

  async function apply() {
    setBusy(true);
    setMsg(null);
    try {
      await assignRole(userId, role, scope);
      setMsg("Applied.");
    } catch (e) {
      const ae = e as ApiError;
      setMsg(`Error: ${ae.detail ?? ae.message}`);
    } finally {
      setBusy(false);
    }
  }

  return (
    <div>
      <h4>Assign role</h4>
      <p className="kpi-sub">
        Applying a role writes its grants to this user now, at the scope chosen below. It is a
        one-time template, not a live link: editing the role's definition later does not reach
        users already assigned it.
      </p>
      <div className="controls">
        <label>Role{" "}
          <select value={role} onChange={(e) => setRole(e.target.value)}>
            {ADMIN_ROLES.map((r) => <option key={r.value} value={r.value}>{r.label}</option>)}
          </select>
        </label>
        <label>Scope{" "}
          <select
            value={scope === null ? "" : scope}
            onChange={(e) => setScope(e.target.value === "" ? null : Number(e.target.value))}
          >
            <option value="">All portfolios</option>
            {portfolios.map((p) => <option key={p.id} value={p.id}>{p.name}</option>)}
          </select>
        </label>
        <button type="button" disabled={busy} onClick={() => void apply()}>Apply</button>
        {msg && <span>{msg}</span>}
      </div>
    </div>
  );
}
