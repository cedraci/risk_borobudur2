/** The register's three states have to be visually distinct, and a denial
 * must never look like an empty register. */
import { screen, waitFor } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import BreachesPage from "./BreachesPage";
import { renderPage, stubFetch, stubFetchStatus, TEST_PORTFOLIO } from "../test/harness";

const RUNS = {
  runs: [{
    id: 1, nav_date: "2026-08-07", run_at: "2026-08-07T09:00:00Z",
    triggered_by: "import", import_id: 3, inputs_complete: true, input_notes: {},
    results: [{ check_key: "issuer_10", scope_label: "Issuer <= 10% NAV",
                limit_value: 0.10, observed_value: 0.106, status: "breach", detail: {} }],
  }],
};

const episode = (over: Record<string, unknown>) => ({
  id: 9, check_key: "issuer_10", subject: "ACME",
  opened_nav_date: "2026-08-07", opened_value: 0.106, peak_value: 0.121,
  peak_nav_date: "2026-08-14", closed_nav_date: null,
  state: "open", classification: "unclassified",
  proposed_classification: "passive",
  proposal_reason: "no purchase in ACME since the previous snapshot",
  acknowledged_at: null, acknowledgement_note: null, deadline_date: null,
  resolved_at: null, resolution_note: null,
  ...over,
});

function stub(breaches: unknown[]) {
  const p = TEST_PORTFOLIO.id;
  stubFetch({
    [`/api/portfolios/${p}/limit-runs`]: RUNS,
    [`/api/portfolios/${p}/breaches`]: { breaches },
  });
}

describe("breach register", () => {
  it("shows the proposal and says it is not yet a decision", async () => {
    stub([episode({})]);
    const { container } = renderPage(<BreachesPage />);
    await waitFor(() => expect(container.textContent).toContain("ACME"));
    expect(screen.getByText(/no purchase in ACME/)).toBeDefined();
    expect(container.textContent).toContain("Unclassified");
  });

  it("distinguishes cleared-awaiting-sign-off from resolved", async () => {
    stub([
      episode({ id: 1, subject: "CLEARED", closed_nav_date: "2026-08-14", state: "acknowledged" }),
      episode({ id: 2, subject: "SIGNEDOFF", closed_nav_date: "2026-08-14", state: "resolved",
                classification: "passive", resolution_note: "trimmed" }),
    ]);
    const { container } = renderPage(<BreachesPage />);
    await waitFor(() => expect(container.textContent).toContain("CLEARED"));
    expect(container.textContent).toMatch(/awaiting sign-off/i);
    expect(container.textContent).toMatch(/Resolved/);
  });

  it("renders a denial rather than an empty register", async () => {
    const p = TEST_PORTFOLIO.id;
    // `stubFetch` answers 200; for a 403 the page must use useFetch's
    // `forbidden`, so this uses `stubFetchStatus` to hand back a real 403.
    stubFetchStatus({
      [`/api/portfolios/${p}/limit-runs`]: { status: 200, body: RUNS },
      [`/api/portfolios/${p}/breaches`]: { status: 403, body: { detail: "not permitted: portfolio settings" } },
    });
    const { container } = renderPage(<BreachesPage />);
    await waitFor(() => expect(container.textContent).toContain("N/A"));
    expect(screen.getByText(/not permitted: portfolio settings/)).toBeDefined();
  });

  it("marks a run whose inputs were incomplete", async () => {
    const p = TEST_PORTFOLIO.id;
    stubFetch({
      [`/api/portfolios/${p}/limit-runs`]: { runs: [{
        ...RUNS.runs[0], inputs_complete: false,
        input_notes: { shareholders: "no register loaded" },
      }] },
      [`/api/portfolios/${p}/breaches`]: { breaches: [] },
    });
    const { container } = renderPage(<BreachesPage />);
    await waitFor(() => expect(container.textContent).toContain("2026-08-07"));
    expect(container.textContent).toMatch(/incomplete/i);
  });
});
