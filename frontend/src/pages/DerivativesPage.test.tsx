/** EMIR's denial contract (finding P3).
 *
 * `emir.rs` nulls every computed number and stamps every class verdict
 * "unavailable" when the reference read behind the OTC classification is
 * denied, and carries the reason in `clearing_obligation`. The page rendered
 * that verdict through the chip's fall-through branch — the same red used for
 * BREACH — and never read the marker, so a permission denial appeared as a
 * clearing-threshold breach with no explanation. `kpis_status` was not read
 * either.
 */
import { screen, waitFor } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import DerivativesPage from "./DerivativesPage";
import { denied, renderPage, stubFetch, TEST_PORTFOLIO } from "../test/harness";

const EXPOSURE = {
  dates: ["2026-08-07"], date: "2026-08-07", aum: 100_000_000,
  categories: [], total: { long: 0, short: 0, gross: 0, net: 0 },
  rows: [], excluded: [], unconfirmed: [], note: "",
  reference_status: { status: "ok" }, nav_status: { status: "ok" },
};

const CLASS_DENIED = {
  class: "credit", label: "Credit", threshold_eur: 1_000_000_000,
  months: [{ month: "2026-08-01", snapshot_date: "2026-08-07", total_eur: null, otc_eur: null }],
  avg_total_eur: null, avg_otc_eur: null, pct_of_threshold: null, verdict: "unavailable",
};

const CLASS_BREACH = {
  class: "credit", label: "Credit", threshold_eur: 1_000_000_000,
  months: [{ month: "2026-08-01", snapshot_date: "2026-08-07", total_eur: 2e9, otc_eur: 2e9 }],
  avg_total_eur: 2e9, avg_otc_eur: 2e9, pct_of_threshold: 2, verdict: "breach",
};

function emirBody(over: Record<string, unknown>) {
  return {
    dates: ["2026-08-07"], date: "2026-08-07",
    months_present: 12, months_total: 12,
    classes: [CLASS_BREACH],
    clearing_obligation: { status: "ok" },
    warnings: [],
    monitors: { otc_open_contracts: 3, reconciliation: "quarterly", compression_required: false },
    monitors_note: "", margin: [], futures_count: 3,
    kpis: [], kpis_status: { status: "ok" },
    otc_note: "",
    ...over,
  };
}

function stub(over: Record<string, unknown>) {
  stubFetch({
    [`/api/portfolios/${TEST_PORTFOLIO.id}/emir`]: emirBody(over),
    [`/api/portfolios/${TEST_PORTFOLIO.id}/derivatives`]: EXPOSURE,
  });
}

describe("EMIR with the OTC classification denied", () => {
  it("names the denial beside the clearing-threshold table", async () => {
    stub({ classes: [CLASS_DENIED], clearing_obligation: denied("reference data") });
    renderPage(<DerivativesPage />);

    await waitFor(() => expect(screen.getByText(/EMIR clearing thresholds/)).toBeDefined());
    expect(screen.getByText(/not permitted: reference data/)).toBeDefined();
  });

  it("does not paint an unavailable verdict as a breach", async () => {
    stub({ classes: [CLASS_DENIED], clearing_obligation: denied("reference data") });
    const { container } = renderPage(<DerivativesPage />);

    await waitFor(() => expect(screen.getByText(/EMIR clearing thresholds/)).toBeDefined());
    const chips = Array.from(container.querySelectorAll("tbody .neg, tbody .warn-badge, tbody .pos"));
    expect(chips.map((c) => c.textContent)).not.toContain("BREACH");
    for (const chip of chips) {
      expect(chip.className).not.toContain("neg");
    }
  });

  it("names the denial on the monthly KPI card", async () => {
    stub({ kpis_status: denied("reference data") });
    renderPage(<DerivativesPage />);

    await waitFor(() => expect(screen.getByText(/Monthly EMIR KPIs/)).toBeDefined());
    expect(screen.getAllByText(/not permitted: reference data/).length).toBeGreaterThan(0);
  });

  it("still paints a real breach red", async () => {
    stub({});
    const { container } = renderPage(<DerivativesPage />);

    await waitFor(() => expect(screen.getByText(/EMIR clearing thresholds/)).toBeDefined());
    expect(screen.queryByText(/not permitted/)).toBeNull();
    const breach = container.querySelector("tbody .neg");
    expect(breach?.textContent).toBe("BREACH");
  });
});
