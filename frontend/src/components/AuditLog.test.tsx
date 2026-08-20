/** The audit log's origin column (finding P7's UI half).
 *
 * The server now records where each request came from. A log that holds the
 * address but never shows it answers no question anyone actually asks of an
 * audit trail, and "which address was hammering the sign-in?" is the first
 * one asked of a run of failures.
 */
import { render, screen, waitFor } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import AuditLog from "./AuditLog";
import { stubFetch } from "../test/harness";

const ROWS = [
  {
    id: 2, at: "2026-08-20T09:12:00Z", actor_label: "ops@f.lu", action: "login_failed",
    domain: null, portfolio_id: null, detail: { email: "ops@f.lu" }, source_addr: "203.0.113.7",
  },
  {
    id: 1, at: "2026-08-20T09:10:00Z", actor_label: "desktop", action: "login",
    domain: null, portfolio_id: null, detail: {}, source_addr: null,
  },
];

describe("audit log", () => {
  it("shows the source address, and a dash where there is none", async () => {
    stubFetch({ "/api/admin/audit": ROWS });
    const { container } = render(<AuditLog />);

    await waitFor(() => expect(screen.getByText("login_failed")).toBeDefined());
    const headers = Array.from(container.querySelectorAll("th")).map((th) => th.textContent);
    expect(headers).toContain("Source");

    expect(screen.getByText("203.0.113.7")).toBeDefined();
    const rows = container.querySelectorAll("tbody tr");
    const lastCells = Array.from(rows[1].querySelectorAll("td")).map((td) => td.textContent);
    expect(lastCells).toContain("—");
  });
});
