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

/** Answers each request with a chosen status code, matched by URL substring.
 * A request no route matches fails the test loudly rather than resolving to
 * `undefined` — a silent 404 would look like an empty page and hide a wrong
 * URL. The one matcher both `stubFetchStatus` and `stubFetch` share, so a
 * page's handling of a non-200 (e.g. a 403) can be tested without
 * hand-rolling a `Response`. */
export function stubFetchStatus(routes: Record<string, { status: number; body: unknown }>) {
  vi.stubGlobal("fetch", async (input: RequestInfo | URL): Promise<Response> => {
    const url = typeof input === "string" ? input : input.toString();
    const hit = Object.keys(routes).find((k) => url.includes(k));
    if (!hit) throw new Error(`no stub for ${url}`);
    const { status, body } = routes[hit];
    return new Response(JSON.stringify(body), {
      status, headers: { "content-type": "application/json" },
    });
  });
}

/** `stubFetchStatus` with every route pinned to 200 — the common case, where
 * the payload shape is what's under test and the status code isn't. */
export function stubFetch(routes: Record<string, unknown>) {
  stubFetchStatus(
    Object.fromEntries(Object.entries(routes).map(([k, body]) => [k, { status: 200, body }])),
  );
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
