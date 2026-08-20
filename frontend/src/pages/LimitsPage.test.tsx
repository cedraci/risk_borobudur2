/** Liquidity's coverage line (finding P3, fourth unread marker).
 *
 * `limits.rs` reports the shareholder register under `coverage.register` with
 * its own `status`/`reason`, because a denied register reads as an *empty*
 * one: `count: 0`, `stale: false`. The page rendered only `stale`, so a
 * denial appeared as a register that had simply never been uploaded. The
 * top-5 scenario is already marked server-side; this is about the coverage
 * line beside it agreeing.
 */
import { screen, waitFor } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import LimitsPage from "./LimitsPage";
import { denied, renderPage, stubFetch, TEST_PORTFOLIO } from "../test/harness";

vi.mock("echarts", () => ({
  init: () => ({ setOption: () => {}, resize: () => {}, dispose: () => {} }),
}));

const BUCKETS = [
  { bucket: "d1", weight: 0.5 }, { bucket: "d2_7", weight: 0.2 },
  { bucket: "d8_30", weight: 0.2 }, { bucket: "d30p", weight: 0.1 },
];

const CONCENTRATION = {
  dates: ["2026-08-07"], date: "2026-08-07", checks: [], excluded_note: "",
  issuer_overrides: { status: "ok" },
};

const RATES = {
  dates: ["2026-08-07"], date: "2026-08-07", bonds: [],
  dv01_total: 0, nav_sensitivity_100bp: null, aum: 1, missing_any: false,
  futures_no_spec: [], futures_no_ctd: [], futures: [],
  reference_status: { status: "ok" }, note: "",
};

const FLOWS = { n_observations: 0, dates_excluded_no_nav: 0 };

function liquidity(register: Record<string, unknown>) {
  return {
    dates: ["2026-08-07"], date: "2026-08-07", nav: 100_000_000,
    params: {
      participation_rate: 0.3, adv_stress_factor: 0.5, liquidity_horizon_days: 5,
      settlement_deadline_days: 2, adv_max_age_days: 7, redemption_shock: 0.1, day_unit: "days",
    },
    coverage: {
      adv_pct_of_nav: 0.8, fallbacks: [], coupon_gaps: [],
      register,
    },
    asset: { normal: { buckets: BUCKETS, cumulative: BUCKETS }, stressed: { buckets: BUCKETS, cumulative: BUCKETS } },
    scenarios: [],
    negative_memo: 0, negative_memo_eur: 0,
    issuer_overrides: { status: "ok" }, nav_status: { status: "ok" },
  };
}

function stub(register: Record<string, unknown>) {
  const p = TEST_PORTFOLIO.id;
  stubFetch({
    [`/api/portfolios/${p}/metrics/concentration`]: CONCENTRATION,
    [`/api/portfolios/${p}/metrics/liquidity`]: liquidity(register),
    [`/api/portfolios/${p}/metrics/rates`]: RATES,
    [`/api/portfolios/${p}/flows`]: FLOWS,
  });
}

describe("liquidity coverage with the shareholder register denied", () => {
  it("distinguishes a denied register from one that was never uploaded", async () => {
    stub({ count: 0, as_of: null, stale: false, ...denied("shareholder register") });
    const { container } = renderPage(<LimitsPage />);

    await waitFor(() => expect(container.textContent).toContain("ADV measured on"));
    expect(screen.getByText(/not permitted: shareholder register/)).toBeDefined();
  });

  it("says nothing extra when the register is readable", async () => {
    stub({ count: 5, as_of: "2026-08-01", stale: false, status: "ok" });
    const { container } = renderPage(<LimitsPage />);

    await waitFor(() => expect(container.textContent).toContain("ADV measured on"));
    expect(screen.queryByText(/not permitted/)).toBeNull();
  });
});
