import { BrowserRouter, NavLink, Route, Routes } from "react-router-dom";
import Overview from "./pages/Overview";
import Performance from "./pages/Performance";
import Risk from "./pages/Risk";
import VarPage from "./pages/VarPage";
import LimitsPage from "./pages/LimitsPage";
import DataPage from "./pages/DataPage";

const links = [
  { to: "/", label: "Overview" },
  { to: "/performance", label: "Performance" },
  { to: "/risk", label: "Risk" },
  { to: "/var", label: "VaR / ES" },
  { to: "/limits", label: "Limits" },
  { to: "/data", label: "Data" },
];

export default function App() {
  return (
    <BrowserRouter>
      <div className="layout">
        <nav className="sidebar">
          <h1>Borobudur<br />Risk</h1>
          {links.map((l) => (
            <NavLink key={l.to} to={l.to} end={l.to === "/"}>{l.label}</NavLink>
          ))}
        </nav>
        <main className="content">
          <Routes>
            <Route path="/" element={<Overview />} />
            <Route path="/performance" element={<Performance />} />
            <Route path="/risk" element={<Risk />} />
            <Route path="/var" element={<VarPage />} />
            <Route path="/limits" element={<LimitsPage />} />
            <Route path="/data" element={<DataPage />} />
          </Routes>
        </main>
      </div>
    </BrowserRouter>
  );
}
