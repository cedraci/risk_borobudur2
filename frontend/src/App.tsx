import { useEffect } from "react";
import {
  BrowserRouter, Navigate, NavLink, Route, Routes, useLocation, useNavigate, useParams,
} from "react-router-dom";
import { getPortfolios, type Portfolio } from "./api";
import { useFetch } from "./hooks";
import { lastPortfolio, PortfolioContext, rememberPortfolio } from "./PortfolioContext";
import Overview from "./pages/Overview";
import Performance from "./pages/Performance";
import PnlPage from "./pages/PnlPage";
import Risk from "./pages/Risk";
import VarPage from "./pages/VarPage";
import LimitsPage from "./pages/LimitsPage";
import DerivativesPage from "./pages/DerivativesPage";
import DataPage from "./pages/DataPage";

const links = [
  { to: "", label: "Overview" },
  { to: "/performance", label: "Performance" },
  { to: "/pnl", label: "P&L" },
  { to: "/risk", label: "Risk" },
  { to: "/var", label: "VaR / ES" },
  { to: "/limits", label: "Limits" },
  { to: "/derivatives", label: "Derivatives" },
  { to: "/data", label: "Data" },
];

/** `/` has no portfolio of its own — send the user into the remembered one
 * (falling back to the first active portfolio when it's missing or archived). */
function RootRedirect({ portfolios }: { portfolios: Portfolio[] }) {
  const active = portfolios.filter((p) => !p.archived);
  if (active.length === 0) {
    return (
      <div className="layout">
        <main className="content">
          <p>No active portfolios yet.</p>
        </main>
      </div>
    );
  }
  const remembered = lastPortfolio();
  const target = active.find((p) => p.id === remembered) ?? active[0];
  return <Navigate to={`/p/${target.id}/`} replace />;
}

function PortfolioLayout({ portfolios }: { portfolios: Portfolio[] }) {
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
          {links.map((l) => (
            <NavLink key={l.to} to={`${prefix}${l.to}`} end={l.to === ""}>{l.label}</NavLink>
          ))}
        </nav>
        <main className="content">
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

export default function App() {
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
    <BrowserRouter>
      <Routes>
        <Route path="/" element={<RootRedirect portfolios={portfolios.data} />} />
        <Route path="/p/:pid/*" element={<PortfolioLayout portfolios={portfolios.data} />} />
        <Route path="*" element={<Navigate to="/" replace />} />
      </Routes>
    </BrowserRouter>
  );
}
