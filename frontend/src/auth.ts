import { req } from "./api";

/** Mirrors `crates/server/src/handlers/session.rs::Capability` — `domain`/`action`
 * are the `Domain`/`Action` string reprs (`db::auth::model`, e.g. "positions"/"view"),
 * `portfolio_id: null` means the grant applies to every portfolio. */
export interface Capability {
  domain: string;
  action: string;
  portfolio_id: number | null;
}

/** Mirrors `MeResponse` from `crates/server/src/handlers/session.rs`. */
export interface Me {
  display_name: string;
  is_administrator: boolean;
  capabilities: Capability[];
}

export const fetchMe = () => req<Me>("/api/me");

export const login = (email: string, password: string) =>
  req<{ ok: boolean }>("/api/login", {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ email, password }),
  }, {
    // A bad-credentials 401 here is an expected outcome of the login form, not a sign a
    // previously-valid session died — don't fire the app-wide "drop to login" event over it.
    suppressUnauthenticatedEvent: true,
  });

export const logout = () => req<void>("/api/logout", { method: "POST" });

export function can(me: Me | null, domain: string, action: string, portfolioId?: number): boolean {
  if (!me) return false;
  return me.capabilities.some(c =>
    c.domain === domain && c.action === action &&
    (c.portfolio_id === null || c.portfolio_id === portfolioId));
}

/** Desktop mode's identity is hardcoded in `crates/server/src/auth/desktop.rs`
 * (`DesktopSingleUser`) — a fixed principal that `/api/me` always resolves to
 * regardless of any cookie, with `display_name: "desktop"`. There is no `mode` field on
 * `Me` to check instead (adding one is a backend change, out of scope for a
 * frontend-only fix) — this name match is the only signal the frontend has. It is a
 * heuristic, not a guarantee: a server-mode account literally named "desktop" would
 * also be treated as the desktop principal. Used only to hide the sign-out control,
 * where the failure mode of that heuristic being wrong is a spurious button, not a
 * security decision. */
export function isDesktopPrincipal(me: Me): boolean {
  return me.display_name === "desktop";
}
