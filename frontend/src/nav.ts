import { can, type Me } from "./auth";

// Each nav destination's required grant(s), per the authorization matrix in
// crates/server/tests/api_authz_matrix.rs — a link is hidden when the principal
// has none of them on the current portfolio, since every page behind it would
// otherwise render only <Unavailable/>. Most destinations map to one dominant
// (domain, action); Data touches several domains at once (imports, reference,
// the shareholder register) with no single one of them dominant, so it's
// visible if the user has any grant that would make at least one of its cards
// useful. Pages that touch a second, weaker domain beyond what's listed here
// (e.g. Limits' shareholder flows) still degrade section-by-section via
// <Unavailable/> rather than hiding the whole tab.
export interface NavSpec { to: string; label: string; requires: { domain: string; action: string }[] }
export const NAV_LINKS: NavSpec[] = [
  { to: "", label: "Overview", requires: [{ domain: "nav", action: "view" }] },
  { to: "/performance", label: "Performance", requires: [{ domain: "nav", action: "view" }] },
  { to: "/pnl", label: "P&L", requires: [{ domain: "positions", action: "view" }] },
  { to: "/risk", label: "Risk", requires: [{ domain: "nav", action: "view" }] },
  { to: "/var", label: "VaR / ES", requires: [{ domain: "nav", action: "view" }] },
  { to: "/limits", label: "Limits", requires: [{ domain: "positions", action: "view" }] },
  { to: "/breaches", label: "Breaches", requires: [{ domain: "settings", action: "view" }] },
  { to: "/derivatives", label: "Derivatives", requires: [{ domain: "positions", action: "view" }] },
  {
    to: "/data", label: "Data",
    requires: [
      { domain: "reference", action: "view" },
      { domain: "positions", action: "view" },
      { domain: "positions", action: "import" },
      { domain: "shareholders", action: "view" },
      // The futures contract table and the weekly CTD upload sit on this page
      // and are gated on `market_data`; without these two a principal granted
      // exactly that job sees no tab at all, though the endpoints behind it
      // would authorize them (finding P5).
      { domain: "market_data", action: "view" },
      { domain: "market_data", action: "import" },
      // The settings card, the depositary code mapping and the import ledger
      // moved to their own domain in the P10 split; the tab that hosts them
      // has to recognise it (finding P10).
      { domain: "settings", action: "view" },
    ],
  },
];

/** The sidebar for one principal on one portfolio. Exported so the gating
 * rule can be pinned directly (src/App.test.tsx) instead of only through a
 * fully mounted app. */
export function visibleNavLinks(me: Me, portfolioId: number): NavSpec[] {
  return NAV_LINKS.filter((l) => l.requires.some((r) => can(me, r.domain, r.action, portfolioId)));
}
