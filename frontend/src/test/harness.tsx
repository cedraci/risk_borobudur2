/** Shared setup for the UI-contract tests.
 *
 * These tests drive real components against real wire payloads — the only
 * thing stubbed is `fetch`, because the payload shape is precisely what is
 * under test. Nothing in `src/` is mocked: a test that mocked `useFetch` or
 * `api.ts` would pass even if the component read the wrong field, which is
 * the class of bug these exist to catch.
 */
import { render } from "@testing-library/react";
import type { ReactElement } from "react";
import { MemoryRouter } from "react-router-dom";
import { PortfolioContext } from "../PortfolioContext";
import type { Portfolio } from "../api";

export const TEST_PORTFOLIO: Portfolio = {
  id: 7,
  name: "Test Fund",
  kind: "ucits",
  archived: false,
  latest_nav_date: "2026-08-07",
};

/** Answers each request from `routes`, matched by URL substring. A request no
 * route matches fails the test loudly rather than resolving to `undefined` —
 * a silent 404 would look like an empty page and hide a wrong URL. */
export function stubFetch(routes: Record<string, unknown>) {
  const fetchStub = async (input: RequestInfo | URL): Promise<Response> => {
    const url = typeof input === "string" ? input : input.toString();
    const hit = Object.keys(routes).find((k) => url.includes(k));
    if (!hit) throw new Error(`no stub for ${url}`);
    const body = routes[hit];
    return new Response(JSON.stringify(body), {
      status: 200,
      headers: { "content-type": "application/json" },
    });
  };
  vi.stubGlobal("fetch", fetchStub);
}

/** Renders a page as the router would: inside its portfolio context. */
export function renderPage(ui: ReactElement) {
  return render(
    <MemoryRouter>
      <PortfolioContext.Provider value={TEST_PORTFOLIO}>{ui}</PortfolioContext.Provider>
    </MemoryRouter>,
  );
}

/** The wire shape of a denied secondary domain, as `db::auth::Denied::reason`
 * renders it (`crates/db/src/auth/access.rs`). */
export const denied = (label: string) => ({ status: "unavailable", reason: `not permitted: ${label}` });
