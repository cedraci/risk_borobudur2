/** P&L's denial contract (finding P1).
 *
 * With `transactions/view` denied the server can no longer attribute anything
 * to a trade, so realized P&L computes to exactly zero for every instrument
 * and unrealized absorbs the whole period. Rendering those two numbers as
 * ordinary values tells a reader the book traded nothing — a denial wearing
 * the costume of a result, which is the one thing the wrapping feature exists
 * to prevent. The split must read as unavailable, and the reason must be on
 * screen next to it, not only in a warning badge at the foot of the page.
 */
import { screen, waitFor } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import PnlPage from "./PnlPage";
import { denied, renderPage, stubFetch, TEST_PORTFOLIO } from "../test/harness";

/** `fr-FR` number formatting separates groups with U+202F (narrow no-break
 * space), so a literal " " never matches what is actually on screen. */
const norm = (s: string) => s.replace(/[\s  ]+/g, " ");

const GROUP = {
  key: "Equity",
  realized_price: 0, unrealized_price: 1_000_000,
  realized_fx: 0, unrealized_fx: 25_000,
  realized: 0, unrealized: 1_025_000, fx: 25_000, total: 1_025_000,
  instruments: [],
};

const RECON = {
  investment_pnl: 1_025_000, cash_and_margin: 0, accrued_fees: 0,
  provisions: 0, dividend_income: 0, total_pnl: 1_025_000,
  aum_change: 1_025_000, net_flows: 0,
  residual: 0, gross: 1_025_000, within_tolerance: true,
};

function pnlBody(transactionDetail: unknown, group: unknown) {
  return {
    empty: false,
    period: {
      requested_from: "2026-01-01", requested_to: "2026-08-07",
      actual_from: "2026-01-01", actual_to: "2026-08-07", snapshots: 12,
    },
    groups: [group],
    reconciliation: RECON,
    unclassified: 0,
    transaction_detail: transactionDetail,
    warnings: [],
  };
}

describe("P&L with trade history denied", () => {
  it("names the denial where the split is shown, not only in a warning", async () => {
    stubFetch({
      [`/api/portfolios/${TEST_PORTFOLIO.id}/pnl`]: pnlBody(
        denied("transactions"),
        { ...GROUP, realized: null, unrealized: null, realized_price: null, unrealized_price: null },
      ),
    });
    renderPage(<PnlPage />);

    await waitFor(() => expect(screen.getByText(/Equity/)).toBeDefined());
    expect(screen.getByText(/not permitted: transactions/)).toBeDefined();
  });

  it("never prints a realized figure the server could not attribute", async () => {
    stubFetch({
      [`/api/portfolios/${TEST_PORTFOLIO.id}/pnl`]: pnlBody(
        denied("transactions"),
        { ...GROUP, realized: null, unrealized: null, realized_price: null, unrealized_price: null },
      ),
    });
    const { container } = renderPage(<PnlPage />);

    await waitFor(() => expect(screen.getByText(/Equity/)).toBeDefined());
    // The groups table only — the reconciliation table below it legitimately
    // shows 0 € on lines that really are zero.
    const groupsTable = container.querySelectorAll("table")[0];
    const cells = Array.from(groupsTable.querySelectorAll("tbody tr td")).map((td) => norm(td.textContent ?? ""));
    // "0 €" in the realized column is the exact false reading this guards.
    expect(cells.some((c) => /^0[\s ]*€$/.test(c.trim()))).toBe(false);
    // Total is still real — it does not depend on the trade journal.
    expect(cells.some((c) => c.includes("1 025 000"))).toBe(true);
  });

  it("shows the split normally when trade history is granted", async () => {
    stubFetch({
      [`/api/portfolios/${TEST_PORTFOLIO.id}/pnl`]: pnlBody({ status: "ok" }, {
        ...GROUP, realized: 400_000, unrealized: 625_000,
      }),
    });
    const { container } = renderPage(<PnlPage />);

    await waitFor(() => expect(screen.getByText(/Equity/)).toBeDefined());
    expect(screen.queryByText(/not permitted/)).toBeNull();
    expect(norm(container.textContent ?? "")).toContain("400 000");
  });
});
