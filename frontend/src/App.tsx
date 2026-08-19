import { createContext, useContext, useEffect, useState } from "react";
import {
  BrowserRouter, Link, Navigate, NavLink, Route, Routes, useLocation, useNavigate, useParams,
} from "react-router-dom";
import { can, fetchMe, type Me } from "./auth";
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

/** The signed-in principal, provided once by `AuthGate` below. Never null inside
 * the routed app — `AuthGate` only renders it once `/api/me` has resolved. */
const MeContext = createContext<Me | null>(null);
export function useMe(): Me {
  const me = useContext(MeContext);
  if (!me) throw new Error("useMe outside AuthGate");
  return me;
}

// Each nav destination's dominant (domain, action) grant, per the authorization
// matrix in crates/server/tests/api_authz_matrix.rs — a link is hidden when the
// principal has no grant for it on any portfolio, since every page behind it
// would otherwise render only <Unavailable/>. Pages that touch a second, weaker
// domain (e.g. Limits' shareholder flows, Data's shareholder register) still
// degrade section-by-section via <Unavailable/> rather than hiding the whole tab.
const links: { to: string; label: string; domain: string; action: string }[] = [
  { to: "", label: "Overview", domain: "nav", action: "view" },
  { to: "/performance", label: "Performance", domain: "nav", action: "view" },
  { to: "/pnl", label: "P&L", domain: "positions", action: "view" },
  { to: "/risk", label: "Risk", domain: "nav", action: "view" },
  { to: "/var", label: "VaR / ES", domain: "nav", action: "view" },
  { to: "/limits", label: "Limits", domain: "positions", action: "view" },
  { to: "/derivatives", label: "Derivatives", domain: "positions", action: "view" },
  { to: "/data", label: "Data", domain: "reference", action: "view" },
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
  const me = useMe();
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
  const visibleLinks = links.filter((l) => can(me, l.domain, l.action, portfolio.id));

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

  return (
    <MeContext.Provider value={me}>
      <AuthedApp />
    </MeContext.Provider>
  );
}

export default function App() {
  return (
    <BrowserRouter>
      <AuthGate />
    </BrowserRouter>
  );
}
