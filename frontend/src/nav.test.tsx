/** Sidebar gating (finding P5).
 *
 * A tab is hidden when the principal holds no grant that would make anything
 * behind it useful. The rule has to stay in step with what each page actually
 * gates on: the futures contract table and the weekly cheapest-to-deliver
 * upload live on the Data page and are gated on `market_data`, so a
 * market-data principal that sees no Data tab is locked out of the one job
 * their grants authorize.
 */
import { describe, expect, it } from "vitest";
import { visibleNavLinks } from "./nav";
import type { Capability, Me } from "./auth";

const PID = 7;

function principal(...caps: [string, string, number | null][]): Me {
  return {
    display_name: "Test",
    is_administrator: false,
    capabilities: caps.map(([domain, action, portfolio_id]): Capability => ({ domain, action, portfolio_id })),
  };
}

const labels = (me: Me) => visibleNavLinks(me, PID).map((l) => l.label);

describe("sidebar gating", () => {
  it("offers Data to a market-data-only principal", () => {
    const me = principal(["market_data", "view", PID], ["market_data", "import", PID]);
    expect(labels(me)).toContain("Data");
  });

  it("offers nothing at all to a principal with no grant on this portfolio", () => {
    const me = principal(["positions", "view", 99]);
    expect(labels(me)).toEqual([]);
  });

  it("gives a nav-only principal the NAV pages and nothing position-derived", () => {
    const me = principal(["nav", "view", PID]);
    expect(labels(me)).toEqual(["Overview", "Performance", "Risk", "VaR / ES"]);
  });

  it("gives a positions-view principal the position-derived pages and Data", () => {
    const me = principal(["positions", "view", PID]);
    expect(labels(me)).toEqual(["P&L", "Limits", "Derivatives", "Data"]);
  });

  it("treats an all-portfolios grant as covering this portfolio", () => {
    const me = principal(["shareholders", "view", null]);
    expect(labels(me)).toContain("Data");
  });
});
