import { createContext, useContext, useEffect, useState } from "react";
import {
  BrowserRouter, Link, Navigate, NavLink, Route, Routes, useLocation, useNavigate, useParams,
} from "react-router-dom";
import { can, fetchMe, isDesktopPrincipal, logout, type Me } from "./auth";
import { getPortfolios, type Portfolio } from "./api";
import { useFetch } from "./hooks";
import { lastPortfolio, PortfolioContext, PortfoliosReloadContext, rememberPortfolio } from "./PortfolioContext";
import LoginPage from "./pages/LoginPage";
import Overview from "./pages/Overview";
import Performance from "./pages/Performance";
import PnlPage from "./pages/PnlPage";
import Risk from "./pages/Risk";
import VarPage from "./pages/VarPage";
import LimitsPage from "./pages/LimitsPage";
import DerivativesPage from "./pages/DerivativesPage";
import DataPage from "./pages/DataPage";

/** The signed-in principal plus a way to end the session, provided once by
 * `AuthGate` below. Never null inside the routed app — `AuthGate` only renders
 * it once `/api/me` has resolved. */
interface AuthHandle { me: Me; signOut: () => void }
const AuthContext = createContext<AuthHandle | null>(null);
function useAuth(): AuthHandle {
  const a = useContext(AuthContext);
  if (!a) throw new Error("useAuth outside AuthGate");
  return a;
}
export function useMe(): Me {
  return useAuth().me;
}

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
interface NavSpec { to: string; label: string; requires: { domain: string; action: string }[] }
const links: NavSpec[] = [
  { to: "", label: "Overview", requires: [{ domain: "nav", action: "view" }] },
  { to: "/performance", label: "Performance", requires: [{ domain: "nav", action: "view" }] },
  { to: "/pnl", label: "P&L", requires: [{ domain: "positions", action: "view" }] },
  { to: "/risk", label: "Risk", requires: [{ domain: "nav", action: "view" }] },
  { to: "/var", label: "VaR / ES", requires: [{ domain: "nav", action: "view" }] },
  { to: "/limits", label: "Limits", requires: [{ domain: "positions", action: "view" }] },
  { to: "/derivatives", label: "Derivatives", requires: [{ domain: "positions", action: "view" }] },
  {
    to: "/data", label: "Data",
    requires: [
      { domain: "reference", action: "view" },
      { domain: "positions", action: "view" },
      { domain: "positions", action: "import" },
      { domain: "shareholders", action: "view" },
    ],
  },
];

/** `/` has no portfolio of its own — send the user into the remembered one
 * (falling back to the first active portfolio when it's missing or archived). */
function RootRedirect({ portfolios }: { portfolios: Portfolio[] }) {
  const active = portfolios.filter((p) => !p.archived);
  if (active.length === 0) {
    const first = portfolios[0];
    return (
      <div className="layout">
        <main className="content">
          <p>No active portfolios yet.</p>
          {first && <p><Link to={`/p/${first.id}/data`}>Manage portfolios</Link></p>}
        </main>
      </div>
    );
  }
  const remembered = lastPortfolio();
  const target = active.find((p) => p.id === remembered) ?? active[0];
  return <Navigate to={`/p/${target.id}/`} replace />;
}

function PortfolioLayout({ portfolios }: { portfolios: Portfolio[] }) {
  const { me, signOut } = useAuth();
  const { pid } = useParams<{ pid: string }>();
  const location = useLocation();
  const navigate = useNavigate();
  const portfolio = portfolios.find((p) => String(p.id) === pid);

  useEffect(() => {
    if (portfolio) rememberPortfolio(portfolio.id);
  }, [portfolio]);

  if (!portfolio) return <Navigate to="/" replace />;

  const prefix = `/p/${portfolio.id}`;
  const rel = location.pathname.startsWith(prefix) ? location.pathname.slice(prefix.length) : "";
  const active = portfolios.filter((p) => !p.archived);
  const visibleLinks = links.filter((l) => l.requires.some((r) => can(me, r.domain, r.action, portfolio.id)));

  return (
    <PortfolioContext.Provider value={portfolio}>
      <div className="layout">
        <nav className="sidebar">
          <h1>Borobudur<br />Risk</h1>
          <div className="controls">
            <select
              value={portfolio.id}
              onChange={(e) => navigate(`/p/${e.target.value}${rel}`)}
            >
              {active.map((p) => (
                <option key={p.id} value={p.id}>{p.name}{p.kind === "mandate" ? " (mandat)" : ""}</option>
              ))}
            </select>
          </div>
          {visibleLinks.map((l) => (
            <NavLink key={l.to} to={`${prefix}${l.to}`} end={l.to === ""}>{l.label}</NavLink>
          ))}
          {/* Desktop mode's principal has no real session to end (/api/me always
              resolves regardless of any cookie) — nothing to sign out of. */}
          {!isDesktopPrincipal(me) && (
            <div className="sidebar-user">
              <span title={me.display_name}>{me.display_name}</span>
              <button type="button" onClick={signOut}>Sign out</button>
            </div>
          )}
        </nav>
        <main className="content" key={portfolio.id}>
          <Routes>
            <Route path="" element={<Overview />} />
            <Route path="performance" element={<Performance />} />
            <Route path="pnl" element={<PnlPage />} />
            <Route path="risk" element={<Risk />} />
            <Route path="var" element={<VarPage />} />
            <Route path="limits" element={<LimitsPage />} />
            <Route path="derivatives" element={<DerivativesPage />} />
            <Route path="data" element={<DataPage />} />
          </Routes>
        </main>
      </div>
    </PortfolioContext.Provider>
  );
}

function AuthedApp() {
  const portfolios = useFetch(() => getPortfolios(), []);

  if (portfolios.error) {
    return (
      <div className="layout">
        <main className="content"><p className="neg">{portfolios.error}</p></main>
      </div>
    );
  }
  if (!portfolios.data) {
    return (
      <div className="layout">
        <main className="content"><p>Loading…</p></main>
      </div>
    );
  }

  return (
    <PortfoliosReloadContext.Provider value={portfolios.reload}>
      <Routes>
        <Route path="/" element={<RootRedirect portfolios={portfolios.data} />} />
        <Route path="/p/:pid/*" element={<PortfolioLayout portfolios={portfolios.data} />} />
        <Route path="*" element={<Navigate to="/" replace />} />
      </Routes>
    </PortfoliosReloadContext.Provider>
  );
}

/** Gates the whole app on `/api/me`. Rendered inside <BrowserRouter> (see App
 * below) so swapping between <LoginPage/> and the routed app never touches the
 * browser URL — a session drop mid-session, and the re-login that follows it,
 * lands the user back exactly where they were instead of bouncing them to "/". */
function AuthGate() {
  // undefined = still loading, null = unauthenticated, Me = signed in.
  const [me, setMe] = useState<Me | null | undefined>(undefined);

  useEffect(() => {
    let alive = true;
    fetchMe().then(
      (m) => alive && setMe(m),
      () => alive && setMe(null),
    );
    return () => { alive = false; };
  }, []);

  useEffect(() => {
    function onUnauthenticated() { setMe(null); }
    window.addEventListener("borobudur:unauthenticated", onUnauthenticated);
    return () => window.removeEventListener("borobudur:unauthenticated", onUnauthenticated);
  }, []);

  if (me === undefined) return null;
  if (me === null) return <LoginPage onLogin={setMe} />;

  // Best-effort: whether or not the server round-trip succeeds, the point of
  // clicking "sign out" is to stop being signed in locally, so drop back to the
  // login screen either way rather than stranding the user on a failed request.
  async function signOut() {
    try { await logout(); } catch { /* the cookie is gone from here regardless */ }
    setMe(null);
  }

  return (
    <AuthContext.Provider value={{ me, signOut: () => void signOut() }}>
      <AuthedApp />
    </AuthContext.Provider>
  );
}

export default function App() {
  return (
    <BrowserRouter>
      <AuthGate />
    </BrowserRouter>
  );
}
