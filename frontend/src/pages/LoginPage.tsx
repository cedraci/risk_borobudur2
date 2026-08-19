import { useState } from "react";
import { ApiError } from "../api";
import { fetchMe, login, type Me } from "../auth";

/** Rendered by App.tsx's AuthGate whenever `/api/me` comes back 401 — either at
 * mount or later, from the `borobudur:unauthenticated` event a session drop fires.
 * AuthGate stays mounted inside the same <BrowserRouter>, so the URL the user was
 * on is untouched by this swap and `onLogin` just resumes rendering it. */
export default function LoginPage({ onLogin }: { onLogin: (me: Me) => void }) {
  const [email, setEmail] = useState("");
  const [password, setPassword] = useState("");
  const [busy, setBusy] = useState(false);
  const [err, setErr] = useState<string | null>(null);

  async function submit(e: React.FormEvent) {
    e.preventDefault();
    setBusy(true);
    setErr(null);
    try {
      await login(email, password);
      onLogin(await fetchMe());
    } catch (x) {
      if (x instanceof ApiError) {
        setErr(x.detail ?? x.message ?? "Sign-in failed.");
      } else {
        // fetch() itself rejected (offline, DNS failure, server unreachable) before a
        // response ever came back, so there's no ApiError to read a reason from.
        setErr("Could not reach the server — check your connection and try again.");
      }
    } finally {
      setBusy(false);
    }
  }

  return (
    <div className="login-shell">
      <form className="card login-card" onSubmit={(e) => void submit(e)}>
        <h2>Borobudur Risk</h2>
        <div className="controls">
          <label>Email
            <input
              type="email" value={email} autoFocus required disabled={busy}
              onChange={(e) => setEmail(e.target.value)}
            />
          </label>
          <label>Password
            <input
              type="password" value={password} required disabled={busy}
              onChange={(e) => setPassword(e.target.value)}
            />
          </label>
          {err && <p className="neg">{err}</p>}
          <button type="submit" disabled={busy || !email || !password}>
            {busy ? "Signing in…" : "Sign in"}
          </button>
        </div>
      </form>
    </div>
  );
}
