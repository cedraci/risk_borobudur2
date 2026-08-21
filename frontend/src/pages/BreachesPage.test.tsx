/** The register's three states have to be visually distinct, and a denial
 * must never look like an empty register. */
import { fireEvent, screen, waitFor } from "@testing-library/react";
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

// The full wire shape from `db::repo::breaches::BreachRow` — including the
// two actor-label fields — so a fixture that overrides only one still
// matches what the server actually sends (`test/harness.tsx`'s own
// contract for fixtures).
const episode = (over: Record<string, unknown>) => ({
  id: 9, check_key: "issuer_10", subject: "ACME",
  opened_nav_date: "2026-08-07", opened_value: 0.106, peak_value: 0.121,
  peak_nav_date: "2026-08-14", closed_nav_date: null,
  state: "open", classification: "unclassified",
  proposed_classification: "passive",
  proposal_reason: "no purchase in ACME since the previous snapshot",
  acknowledged_at: null, acknowledgement_note: null, acknowledged_by_label: null,
  deadline_date: null,
  resolved_at: null, resolution_note: null, resolved_by_label: null,
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
    expect(container.textContent).toContain("Proposed:");
  });

  // `proposed_classification` is `null` in three real cases (first snapshot,
  // no holdings, or a quantity missing at one of the two snapshots — see
  // `analytics::breach::propose`), each still carrying a `proposal_reason`.
  // Falling through to "Passive" there manufactures a guess the engine
  // explicitly declined to make.
  it("never invents a Passive proposal when the engine declined to propose one", async () => {
    stub([episode({
      proposed_classification: null,
      proposal_reason: "quantity of GB00B3X7QG63 is not reported at one of the two snapshots, so a purchase cannot be ruled out",
    })]);
    const { container } = renderPage(<BreachesPage />);
    await waitFor(() => expect(container.textContent).toContain("ACME"));
    // Scoped to the proposal line itself: the acknowledge form on this same
    // (open) episode legitimately offers a "Passive" radio choice, so the
    // assertion must not be fooled by that unrelated occurrence.
    const proposal = screen.getByText(/not reported at one of the two snapshots/);
    expect(proposal.textContent).toContain("No proposal");
    expect(proposal.textContent).not.toContain("Passive");
  });

  it("distinguishes cleared-awaiting-sign-off from resolved, and records who acted", async () => {
    stub([
      episode({
        id: 1, subject: "CLEARED", closed_nav_date: "2026-08-14", state: "acknowledged",
        acknowledged_at: "2026-08-15T09:00:00Z", acknowledged_by_label: "J. Dupont",
        acknowledgement_note: "confirmed passive",
      }),
      episode({
        id: 2, subject: "SIGNEDOFF", closed_nav_date: "2026-08-14", state: "resolved",
        classification: "passive", resolution_note: "trimmed",
        acknowledged_at: "2026-08-15T09:00:00Z", acknowledged_by_label: "J. Dupont",
        resolved_at: "2026-08-16T09:00:00Z", resolved_by_label: "M. Martin",
      }),
    ]);
    const { container } = renderPage(<BreachesPage />);
    await waitFor(() => expect(container.textContent).toContain("CLEARED"));

    // Scoped to each episode's own card: a page that attached "awaiting
    // sign-off" to the wrong card, or to both, must fail here even though
    // the two phrases both appear somewhere on the page.
    const clearedCard = screen.getByText(/CLEARED/).closest(".card") as HTMLElement;
    expect(clearedCard.textContent).toMatch(/awaiting sign-off/i);
    expect(clearedCard.textContent).toContain("J. Dupont");

    const signedOffCard = screen.getByText(/SIGNEDOFF/).closest(".card") as HTMLElement;
    expect(signedOffCard.textContent).toMatch(/Resolved/);
    expect(signedOffCard.textContent).not.toMatch(/awaiting sign-off/i);
    expect(signedOffCard.textContent).toContain("M. Martin");
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

  it("posts the classification and note when acknowledging an open episode", async () => {
    const p = TEST_PORTFOLIO.id;
    let acknowledgeBody: unknown = null;
    const fetchMock = async (input: RequestInfo | URL, init?: RequestInit): Promise<Response> => {
      const url = typeof input === "string" ? input : input.toString();
      if (url.includes(`/api/portfolios/${p}/breaches/9/acknowledge`)) {
        acknowledgeBody = JSON.parse(init!.body as string);
        return new Response(null, { status: 204 });
      }
      if (url.includes(`/api/portfolios/${p}/limit-runs`)) {
        return new Response(JSON.stringify(RUNS), { status: 200, headers: { "content-type": "application/json" } });
      }
      if (url.includes(`/api/portfolios/${p}/breaches`)) {
        return new Response(JSON.stringify({ breaches: [episode({})] }), { status: 200, headers: { "content-type": "application/json" } });
      }
      throw new Error(`no stub for ${url}`);
    };
    vi.stubGlobal("fetch", fetchMock);

    renderPage(<BreachesPage />);
    // Waits for the open episode's acknowledge form to mount rather than for
    // "ACME" text, which also appears (legitimately) inside the proposal
    // sentence and would make `getByText` ambiguous.
    await waitFor(() => screen.getByPlaceholderText("Note (required)"));

    fireEvent.click(screen.getByLabelText(/Passive — market movement/));
    fireEvent.change(screen.getByPlaceholderText("Note (required)"), {
      target: { value: "market moved, no trade in the position" },
    });
    fireEvent.click(screen.getByRole("button", { name: "Acknowledge" }));

    await waitFor(() => expect(acknowledgeBody).toEqual({
      classification: "passive", note: "market moved, no trade in the position",
    }));
  });

  it("surfaces the server's 422 message when acknowledging is refused, and keeps the typed note", async () => {
    const p = TEST_PORTFOLIO.id;
    const fetchMock = async (input: RequestInfo | URL): Promise<Response> => {
      const url = typeof input === "string" ? input : input.toString();
      if (url.includes(`/api/portfolios/${p}/breaches/9/acknowledge`)) {
        return new Response(
          JSON.stringify({ detail: "no open breach with that id — it may already be acknowledged or resolved" }),
          { status: 422, headers: { "content-type": "application/json" } },
        );
      }
      if (url.includes(`/api/portfolios/${p}/limit-runs`)) {
        return new Response(JSON.stringify(RUNS), { status: 200, headers: { "content-type": "application/json" } });
      }
      if (url.includes(`/api/portfolios/${p}/breaches`)) {
        return new Response(JSON.stringify({ breaches: [episode({})] }), { status: 200, headers: { "content-type": "application/json" } });
      }
      throw new Error(`no stub for ${url}`);
    };
    vi.stubGlobal("fetch", fetchMock);

    renderPage(<BreachesPage />);
    await waitFor(() => screen.getByPlaceholderText("Note (required)"));

    fireEvent.click(screen.getByLabelText(/Passive — market movement/));
    const note = screen.getByPlaceholderText("Note (required)");
    fireEvent.change(note, { target: { value: "someone else already acted" } });
    fireEvent.click(screen.getByRole("button", { name: "Acknowledge" }));

    await waitFor(() => expect(screen.getByText(/already be acknowledged or resolved/)).toBeDefined());
    expect((note as HTMLTextAreaElement).value).toBe("someone else already acted");
  });

  it("posts the note when resolving an acknowledged episode", async () => {
    const p = TEST_PORTFOLIO.id;
    let resolveBody: unknown = null;
    const fetchMock = async (input: RequestInfo | URL, init?: RequestInit): Promise<Response> => {
      const url = typeof input === "string" ? input : input.toString();
      if (url.includes(`/api/portfolios/${p}/breaches/9/resolve`)) {
        resolveBody = JSON.parse(init!.body as string);
        return new Response(null, { status: 204 });
      }
      if (url.includes(`/api/portfolios/${p}/limit-runs`)) {
        return new Response(JSON.stringify(RUNS), { status: 200, headers: { "content-type": "application/json" } });
      }
      if (url.includes(`/api/portfolios/${p}/breaches`)) {
        return new Response(
          JSON.stringify({ breaches: [episode({ state: "acknowledged", classification: "passive" })] }),
          { status: 200, headers: { "content-type": "application/json" } },
        );
      }
      throw new Error(`no stub for ${url}`);
    };
    vi.stubGlobal("fetch", fetchMock);

    renderPage(<BreachesPage />);
    await waitFor(() => screen.getByPlaceholderText("Resolution note (required)"));

    fireEvent.change(screen.getByPlaceholderText("Resolution note (required)"), {
      target: { value: "confirmed cleared" },
    });
    fireEvent.click(screen.getByRole("button", { name: "Resolve" }));

    await waitFor(() => expect(resolveBody).toEqual({ note: "confirmed cleared" }));
  });

  it("re-run posts the literal empty body and reloads", async () => {
    const p = TEST_PORTFOLIO.id;
    let rerunBody: string | null = null;
    const fetchMock = async (input: RequestInfo | URL, init?: RequestInit): Promise<Response> => {
      const url = typeof input === "string" ? input : input.toString();
      if (url.includes(`/api/portfolios/${p}/limit-runs`) && init?.method === "POST") {
        rerunBody = init.body as string;
        return new Response(JSON.stringify({ run_id: 2, nav_date: "2026-08-07" }), {
          status: 200, headers: { "content-type": "application/json" },
        });
      }
      if (url.includes(`/api/portfolios/${p}/limit-runs`)) {
        return new Response(JSON.stringify(RUNS), { status: 200, headers: { "content-type": "application/json" } });
      }
      if (url.includes(`/api/portfolios/${p}/breaches`)) {
        return new Response(JSON.stringify({ breaches: [] }), { status: 200, headers: { "content-type": "application/json" } });
      }
      throw new Error(`no stub for ${url}`);
    };
    vi.stubGlobal("fetch", fetchMock);

    renderPage(<BreachesPage />);
    await waitFor(() => screen.getByText("2026-08-07"));

    fireEvent.click(screen.getByRole("button", { name: "Re-run checks now" }));

    await waitFor(() => expect(rerunBody).toBe("{}"));
  });
});
