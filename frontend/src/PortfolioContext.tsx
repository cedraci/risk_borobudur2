import { createContext, useContext } from "react";
import type { Portfolio } from "./api";

/** The portfolio the current /p/{id}/… route is scoped to. Provided by the
 * route layout in App.tsx; every page reads it instead of a prop. */
export const PortfolioContext = createContext<Portfolio | null>(null);

export function usePortfolio(): Portfolio {
  const p = useContext(PortfolioContext);
  if (!p) throw new Error("usePortfolio outside /p/{id} routes");
  return p;
}

const LAST_KEY = "borobudur.lastPortfolio";
export function rememberPortfolio(id: number) {
  try { localStorage.setItem(LAST_KEY, String(id)); } catch { /* private mode */ }
}
export function lastPortfolio(): number | null {
  try {
    const v = localStorage.getItem(LAST_KEY);
    return v ? Number(v) : null;
  } catch { return null; }
}
