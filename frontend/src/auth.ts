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
  });

export const logout = () => req<void>("/api/logout", { method: "POST" });

export function can(me: Me | null, domain: string, action: string, portfolioId?: number): boolean {
  if (!me) return false;
  return me.capabilities.some(c =>
    c.domain === domain && c.action === action &&
    (c.portfolio_id === null || c.portfolio_id === portfolioId));
}
