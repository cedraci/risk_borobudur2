# Server deployment, identity, and authorization — design

Date: 2026-08-14
Status: design approved, plan not yet written

## Why this exists

Borobudur is being prepared for sale to asset managers other than its first
user. Two things stand in the way: the tool has no concept of who is using it,
and its features are welded into the application rather than assembled from
parts a second firm could extend.

This spec covers the first. A second spec — see *Relationship to Spec 2* at the
end — covers the financial-risk module manifest.

The requirement, in the user's words, is that access rights be scoped "based on
portfolios and/or on underlying data". That is two independent axes, and the
grant model below keeps them independent.

### The constraint that orders everything

Access rights are currently unenforceable. `main.rs` starts an embedded
PostgreSQL under the user's local data directory, binds `127.0.0.1:8787`, and
opens a browser. The analyst owns the database file and can read it with
`psql`. Any permission check added to the API in that deployment is decoration.

So the deployment shift is not a preliminary to the authorization work. It is
what makes the authorization work mean anything, and it is step one.

## Decisions taken

| Question | Decision |
|---|---|
| Tenancy | Self-hosted, one instance per firm. Isolation is a deployment boundary, not a `WHERE` clause. |
| Identity | A seam. Local accounts ship now; OIDC later behind the same trait. |
| Data scoping | A small fixed set of six named data domains. |
| Enforcement | The `db` crate stops exposing a pool. Unscoped queries fail to compile. |
| Module shape | Deferred to Spec 2 (feature manifest across all financial risks). |

## 1. Architecture

### Deployment

Three changes to the process boundary, all selected once at startup:

- **Database.** `BOROBUDUR_DATABASE_URL` set → connect to that server and run
  migrations. Unset → today's embedded PostgreSQL bootstrap, preserved verbatim
  as `desktop` mode. One code path chosen at startup, not two threaded through
  the application.
- **Bind address.** `BOROBUDUR_BIND`, defaulting to `127.0.0.1:8787`. In server
  mode a firm sets `0.0.0.0:8787` behind their reverse proxy, which terminates
  TLS. **TLS is not implemented in-process.** Every asset manager that
  self-hosts already owns a proxy and a certificate lifecycle; owning TLS here
  would mean owning certificate rotation for a deployment we cannot observe.
- **Browser auto-open.** Suppressed in server mode.

### Identity

A single trait with one job: turn an incoming request into an authenticated
principal, or reject it.

```rust
trait IdentityProvider {
    async fn authenticate(&self, req: &RequestParts) -> Result<Principal, AuthError>;
}
```

Two implementations ship together:

- **`LocalAccounts`** — users in PostgreSQL, Argon2id password hashes, an opaque
  session cookie (`HttpOnly`, `SameSite=Strict`, `Secure` in server mode).
  Sessions are stored server-side so revocation is immediate.
- **`DesktopSingleUser`** — used in `desktop` mode. Returns a fixed principal
  holding every grant.

`DesktopSingleUser` exists from the first commit. The no-authentication path is
therefore a *configured identity*, not a bypass around the identity check.
There is exactly one way into the system, which is what stops the two paths
drifting apart.

OIDC becomes a third implementation later with no downstream change, because
nothing downstream sees anything but a `Principal`.

### Where it plugs in

One Axum middleware layer resolves the principal, loads that principal's
grants, and constructs the `AuthCtx` that section 3's scoped handle demands.
Handlers never see a raw request identity and never see a connection pool.

### Excluded, deliberately

**Multi-tenancy inside one instance.** Serving several firms from one database
would put a `tenant_id` on all 15 tables and a filter on every query, and would
turn backup, restore and migration into per-tenant operations. The failure mode
— one missed filter leaking one asset manager's positions to another — is
invisible in testing because both tenants' data is identical in shape. It is
also the wrong sales answer: "each client gets a dedicated database" ends a
due-diligence question that "separated by application logic" prolongs.

The case that resembles tenancy but is not: a Luxembourg ManCo hosting sub-funds
for several third-party investment managers is one firm, one instance, and the
requirement that each manager sees only their own sub-funds is exactly what the
portfolio-scoped grant model delivers.

Tenancy is not foreclosed. Because section 3 funnels every query through one
scoped handle, adding a tenant dimension later is a change at one seam plus a
migration, not an audit of every call site.

**General HTTP rate limiting.** This is an internal tool behind a firm's reverse
proxy, reachable by a few dozen named employees, not exposed to the internet.
Per-IP limits belong in nginx, which sheds load before a request reaches a Tokio
task. The one real resource-exhaustion vector is already handled: `routes.rs`
caps request bodies at 20 MB.

**Also excluded:** SSO, password-reset-by-email (an administrator resets a
password; a self-hosting firm has an IT desk).

### In scope, because no proxy can do it

**Login attempt throttling.** Once the app binds to something other than
loopback and accepts a password, unlimited attempts are a real vulnerability,
and a reverse proxy cannot help — it sees a valid-looking POST and cannot
distinguish a failed password from a successful one. Argon2id makes each guess
expensive; a determined attacker parallelises. Therefore: a failed-attempt
counter per account, exponential backoff after five failures, a temporary lock
an administrator can clear, and an audit row per attempt. Per account, not per
IP, so a corporate NAT cannot lock out a whole floor.

**A note on the Bloomberg endpoints.** `/api/bloomberg/request` and
`/adv-request` cost money and quota rather than CPU. The standing constraint —
Bloomberg calls only on explicit user action, never on tab open — remains the
control. What belongs alongside it is not a throttle but a guard: the existing
ADV staleness check already knows what is fresh, so a request for data already
held returns it rather than re-fetching. That is a correctness property inside
the feature, not middleware.

## 2. The grant model

A grant is a triple: `(domain, action, scope)`. Domains answer *what data*,
scope answers *whose portfolios*, actions answer *what you may do to it*.

### The six domains

Fixed, defined as a Rust enum, not user-extensible. A firm inventing its own
domains produces a permission model nobody can reason about, and the compile-time
check in section 3 depends on the set being closed.

| Domain | Tables | Rationale |
|---|---|---|
| `positions` | `position_snapshots`, `dividends` | The core holdings picture |
| `nav` | `nav_history` | Performance is often shareable when holdings are not |
| `transactions` | `operations` | Trade-level detail reveals intent, not just state |
| `shareholders` | `shareholders`, `share_class_flows` | Investor identities — the most restricted data in the system |
| `market_data` | `fx_history`, `futures_analytics`, Bloomberg ADV | Licensed third-party data; restricted for contractual reasons |
| `reference` | `settings`, `portfolios`, `portfolio_codes`, `instrument_refs`, `futures_contracts`, `emir_kpis` | Classifications, thresholds, manually entered values |

The `imports` table is the ingest ledger; viewing import history requires `view`
on `reference`.

### The four actions

`view` is implied by each of the others. The implication is resolved once when
grants are loaded, so the runtime check is a flat set lookup.

- **`view`** — see it on screen, and see analytics computed from it.
- **`export`** — take it out of the system as a file. Separate from `view`
  deliberately: "may read the shareholder register on screen, may not download
  it" is a control asset managers ask for, and it is what makes the audit log
  meaningful.
- **`import`** — load data in. An ingest adapter declares which domains its
  `UniversalBatch` writes, and the check requires `import` on all of them, so
  permission follows the adapter's declaration rather than a hardcoded list that
  drifts.
- **`configure`** — change limit thresholds, portfolio definitions, reference
  classifications. Meaningful only on `reference`, but genuinely distinct: "may
  see fund X's VaR, may not move its VaR limit" is the segregation an audit
  expects.

### Scope

Either an explicit set of portfolio ids, or *all portfolios*. The wildcard
matters because funds get launched and a head of risk should not need
re-granting each time.

One rule covers instance-wide resources: **anything not portfolio-scoped
(instrument classifications, futures contracts, FX history) requires a grant
whose scope is `all`.**

### Storage

One row per `(subject, domain, action, portfolio_id)`, with `portfolio_id NULL`
meaning all portfolios. Flat, indexable, and directly answerable: "why can this
person see fund X's positions?" is one query returning the responsible rows.

### Roles are templates, not indirection

`Risk Analyst`, `Head of Risk`, `Operations`, `Auditor` expand into concrete
grant rows at assignment time. Evaluation stays a flat set — no hierarchy to
traverse, no effective-permission mystery.

The cost is explicit: editing a role does not retroactively change people
already assigned it. The administration screen offers "re-apply to the N users
holding this role". At a scale of tens of users, debuggability is worth more
than that convenience.

Suggested defaults, adjustable per firm:

| Role | Grants |
|---|---|
| Risk Analyst | `view`+`export` on `positions`, `nav`, `transactions`, `market_data`, `reference`; no `shareholders`; no `configure` |
| Head of Risk | `view`+`export` on all six domains and `configure` on `reference`, for assigned portfolios |
| Operations | `import` on `positions`, `nav`, `transactions`, `market_data`; `view` only elsewhere |
| Auditor | `view` on all domains, all portfolios; no `export`, no write |

### Administrator is an instance role, not a grant

Managing users, grants, portfolio creation and the audit log cannot itself be a
portfolio-scoped grant without recursion. `administrator` is a flag on the
principal, outside the domain × portfolio grid.

### No deny rules

Grants are purely additive; absence of a grant is denial. Deny-overrides
semantics make the effective permission set non-local — you can no longer answer
"what can this person see" by reading their rows — and that is where permission
bugs live. The cost is that "everything except Fund Z" cannot be written
compactly; it is enumerated instead. Acceptable at this scale.

### Worked example

The liquidity liability-side redemption analysis reads `shareholders`; the
asset-side days-to-liquidate reads `positions` and `market_data`. An analyst
granted the latter two but not `shareholders` opens the Liquidity tab and gets
the full asset-side analysis, with the top-five-shareholder scenarios returning
`unavailable`, reason `"not permitted: shareholder register"`. The feature does
not branch on permissions — it asks for data, and the scoped handle answers or
explains.

## 3. Enforcement mechanics

### The enabling fact

`analytics` depends only on `chrono` and `serde`. No database, no server. The
risk maths already takes plain structs and returns plain structs, so enforcement
lives entirely at the `db`↔`server` boundary and touches no line of VaR,
liquidity, concentration or P&L code. This refactor is broad in surface area and
shallow in depth.

### `db` stops exporting a pool

Today all ~40 functions in `repo.rs` take `pool: &PgPool`, and `PgPool` is
public, so any handler can query anything. The pool becomes private inside a
`Db` struct, and the only route from `Db` to a query is:

```rust
let scoped = db.scope(&auth_ctx);   // AuthCtx is mandatory, not Option
```

Every repo function becomes a method on `Scoped`. There is no other
constructor. A handler wanting positions without an `AuthCtx` has no reachable
function to call — not "fails a check at runtime", but *does not compile*.

### Authorization produces a typed token

Scoping to a principal is not enough; the wrong portfolio could still be
requested. So the portfolio id stops being a bare `i64` and becomes a value
obtainable only by asking:

```rust
let p = scoped.authorize::<Positions, View>(portfolio_id)?;   // -> Access<Positions, View>
let rows = scoped.positions_for(&p, date).await?;             // demands exactly that type
```

`Access<D, A>` is a newtype over the id with zero-sized domain and action
markers. `positions_for` requires `Access<Positions, View>`;
`shareholders_replace` requires `Access<Shareholders, Import>`. Handing a NAV
authorization to a positions query is a type error. The token's existence is the
proof the check ran, so the check cannot be forgotten — only refused.

The action implication is resolved inside `authorize`, not in the type system: a
principal holding `export` on `positions` obtains `Access<Positions, View>`
successfully, because the implication was expanded when grants were loaded.
Read functions therefore always demand the `View` marker, and a caller that
needs both reads and an export asks for each token separately.

### What stays at runtime

Whether portfolio 7 is in a principal's grant set is a runtime question; ids are
runtime values. But it is decided in exactly one function, `authorize`, which
every path passes through. One function can be tested exhaustively and read in
full during a security review — as against forty opportunities to forget.

### The bootstrap escape hatch

Loading a principal's grants requires querying the database before an `AuthCtx`
exists. There is therefore one privileged path: a `db::admin` module used by
exactly three consumers — identity, grant management, and the audit log. It is
named as such, and a test asserts no other module references it. One loud
exception is preferable to a subtle mechanism pretending there is none.

### Targeted improvement, in scope

`repo.rs` is 1,285 lines and has outgrown itself. It splits into
`repo/positions.rs`, `nav.rs`, `transactions.rs`, `shareholders.rs`,
`market_data.rs`, `reference.rs` — the same six names as the domains. This makes
"which domain does this query touch" answerable by file location, so reviewing
what a domain grant exposes means reading one file rather than grepping. The
split happens as part of the signature change, not as a separate churn commit.

### Router constructors

The router loses its plain `.route()` and gains two:

```rust
.protected(path, method, handler, Domain::Positions, Action::View)
.public(path, method, handler)
```

A route cannot be registered without declaring which it is. `/api/health`, the
login endpoint and the static-asset fallback are `.public` and visibly so.
Endpoint coverage becomes structural rather than a test against a list that
drifts.

The declared pair is the endpoint's **primary** domain and action — the minimum
required to reach the handler at all, enforced as `403` or `404`. Endpoints
whose result draws on further domains (liquidity reads `positions`,
`market_data` and `shareholders`) authorize those per component inside the
handler, and a denial there degrades that component to `unavailable` rather than
failing the request. Section 4 governs which of the two applies.

### Blast radius

~40 repo functions gain a parameter; the 11 handler files (3,500 lines total)
gain an authorize call per endpoint; middleware, `AuthCtx` and the admin screens
are new. `analytics` is untouched. The frontend is untouched except for login
and administration.

## 4. Denial semantics, errors, and audit

### The safety rule

> **A computation whose inputs are partially denied returns `unavailable`. It
> never computes on the subset.**

Concentration limits computed over only the positions a user may see would show
a compliant 5/10/40 result on an incomplete book — a false pass on a regulatory
limit, which is worse than no answer. Denial therefore removes whole
*components*; it never trims *inputs* to one. Each component declares the domains
it consumes, and if any is denied the component is unavailable.

This is why the liquidity split works: asset-side and liability-side are two
components reading different domains, not one computation over a subset.

### Four failure kinds, kept distinct

| Situation | Response | Rationale |
|---|---|---|
| Not authenticated, or session expired | `401`; the frontend preserves the current view and re-authenticates | No retry loop, no silent data-free render |
| Portfolio outside the principal's scope | `404` | `403` would confirm the fund exists; the portfolio namespace must not be enumerable |
| Domain or action denied within a visible portfolio | `403` with `{domain, action, portfolio_id}` | Honest status code, greppable in proxy logs |
| Component denied inside a successful composite result | `200`; the component carries `status:"unavailable"`, `reason:"not permitted: …"` | The request succeeded; the other components are real and should render |

The status code reflects whether the **request** succeeded; the `unavailable`
marker reflects whether a **component** is present. The frontend's fetch wrapper
maps a `403` body onto the same visual treatment as an `unavailable` component,
so there is one rendering path in the UI despite two correct wire behaviours.

### Navigation

`GET /api/me` returns the principal and their resolved capabilities, and the
shell hides sections the user cannot view anywhere. This is a courtesy, not a
control: every endpoint enforces independently, and hidden-section logic is
never the only thing between a user and data.

### Audit log

A single append-only table — timestamp, principal, action, domain, portfolio,
detail, source address. No delete endpoint exists.

Recorded:

- every authentication event: success, failure, lockout, administrator reset
- every `export` — the reason `export` is a first-class action
- every `configure` change, with before and after values
- every grant change, including who granted it
- every `import`, tied to the existing `imports` ledger

Not recorded: `view`. Logging every read produces a log nobody reads, and the
interesting questions here are who took data out and who moved a threshold.

Retention is an administrator export; no automatic purge in v1. A firm needing
seven-year retention ships the table to the archive it already operates.

### Non-permission errors are untouched

"No shareholder register loaded" and "not permitted: shareholder register" are
different reasons carried in the same envelope. The refactor must not blur them;
a missing-data reason must never surface as a permission problem, or the
reverse. This gets an explicit test.

### Desktop mode

`DesktopSingleUser` holds every grant, so nothing renders unavailable and the
current workflow is visually identical. The code path is the same one — same
middleware, same authorize calls, same tokens — so there is no bypass branch to
drift.

## 5. Testing and rollout

### Desktop-mode parity

The existing test suite runs unchanged under `DesktopSingleUser`. Identical VaR,
liquidity, P&L and EMIR numbers with the auth machinery in the path is the
regression net for a change touching ~40 repo functions and 11 handler files.

### Test layers

1. **Grant resolution — pure, no database.** Role-template expansion, the
   `view ⊆ export/import/configure` implication, wildcard scope, additive-only
   composition.
2. **`authorize`, exhaustively.** Table-driven across 6 domains × 4 actions ×
   {granted in scope, granted elsewhere, wildcard, no grant} — 96 cases.
3. **Compile-fail tests via `trybuild`.** At minimum: a query attempted without
   an `AuthCtx`, and an `Access<Nav, View>` passed to a positions function. Both
   must fail to compile. Without these, a future refactor re-exports `PgPool`
   and nothing notices.
4. **Endpoint matrix, integration.** Per protected route: granted → `200`;
   portfolio out of scope → `404`; domain denied → `403`; unauthenticated →
   `401`.
5. **The two behavioural tests that matter most.**
   - Partial-denial safety: a principal without `positions` requesting
     concentration receives `unavailable`, and the test asserts specifically
     that the response is *not* a computed compliant result.
   - Reason-channel separation: missing data and denied permission never surface
     as each other.

Plus audit assertions — `export`, `configure` and `import` write a row, `view`
writes none — and login throttling reaching lockout at the fifth failure.

### Test execution

Per-crate, per-binary invocations with explicit timeouts. Never one
workspace-wide run. This applies with more force now that tests need a database.

### Rollout — the first administrator

On first start in server mode with an empty users table, the server creates one
administrator from `BOROBUDUR_ADMIN_EMAIL` and prints a single-use enrolment
token to stdout, valid for one hour. No default password and no seeded
credential. If the token expires, restarting reissues it. Desktop mode never
enrols anyone.

### Migration

New tables only: `users`, `sessions`, `grants`, `roles`, `audit_events`,
`login_attempts`. No existing table changes, so an existing desktop database
upgrades by running the migration and continues working as a single-user
install.

## Relationship to Spec 2

Spec 2 covers the **financial-risk module manifest**: a `Feature` trait and
registry that turn today's hardcoded routes into registered modules a second
firm can extend.

Its scope is all financial risks, not liquidity alone. The families already
exist in `analytics`, unnamed:

| Family | Existing code | Existing endpoints |
|---|---|---|
| Market risk | `var.rs`, `metrics.rs`, `drawdown.rs`, `backtest.rs`, `rates.rs`, `returns.rs`, `futures.rs` | `/metrics/var`, `/rolling`, `/drawdowns`, `/backtest`, `/metrics/rates` |
| Liquidity risk | `liquidity.rs`, `flows.rs` | `/metrics/liquidity` |
| Concentration risk | `concentration.rs` | `/metrics/concentration` |
| Counterparty & derivatives risk | `emir.rs`, derivatives limits | `/metrics/derivatives`, `/emir*` |

Alongside them sit the non-risk modules: Performance & P&L (`pnl.rs`), Data &
reference (ingest, Bloomberg, futures contracts, refs), and Administration.

Two constraints on that design, established here:

1. **A shared kernel is mandatory.** `returns`, `bizdays`, `stats`, `coupons` and
   FX conversion are used across families. Features may depend on the kernel;
   features must never depend on each other. Shared inputs are shared *data
   domains*, not shared features.
2. **The trait must be neutral across families.** Designing it around liquidity's
   shape — on-demand Bloomberg pull, scenario table, coverage verdict — would not
   fit market risk. It must be defined against all four at once.

### Why this spec comes first

Both projects touch every repository function: one to thread the authorization
context through, one to move code behind a feature boundary. In the wrong order
that cost is paid twice, and the feature manifest would have to be retrofitted
with `AuthCtx` afterwards. Authorization first means a feature's declaration of
which domains it reads refers to something that already exists and is enforced.

The honest caveat on Spec 2: it disturbs existing code more than this one and
delivers no visible feature. It is an investment in extensibility and in the
sales conversation, not a capability.
