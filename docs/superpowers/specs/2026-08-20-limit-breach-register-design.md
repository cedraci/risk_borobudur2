# Limit Breach Register — recorded check runs, breach episodes, sign-off — Design

**Date:** 2026-08-20
**Status:** Approved by user (design presented and approved in chat)
**Baseline:** `main` @ `fe8e746` (authz/wrapping review fixes + settings domain split merged)

## Purpose

The Limits page recomputes its checks every time someone opens it and stores
nothing. Close the browser and there is no record that a check ran on a date,
what it said, or what anyone did about it.

UCITS distinguishes an **active** breach — caused by a transaction — from a
**passive** one caused by market movement or flows, and expects remediation
within a reasonable period. What a CSSF inspection, a depositary or an
internal auditor asks for is the *register*: dates, statuses, who reviewed,
what was decided, when it cleared. Not a screenshot of today.

This design adds that register. It computes nothing new: every check already
produces an OK / WATCH / BREACH verdict. It records them, groups consecutive
breaches into episodes a person can actually act on, and puts a sign-off
workflow over the result.

## Decisions

1. **A run is recorded per snapshot date, on import.** Every NAV Recap or
   CACEIS import recomputes the covered checks for the date it just loaded and
   writes one run. No scheduler is involved — the process has none — and the
   register is tied to the data it was struck on. A manual re-run covers the
   case where reference data changed after the import.
2. **A breach is an episode, not a row per run.** A breach that persists for
   six weeks is one thing to remediate, not forty-two. Episodes open when a
   check first breaches for a subject and close on the data when a later run
   finds it OK.
3. **Runs are computed with full inputs, not the caller's.** See
   "The system context" below. This is the one place the register deliberately
   does not behave like an ordinary read.
4. **Active/passive is proposed, never decided, by the machine.** The proposal
   is derived from position changes and shown with its reasoning; a person
   confirms or overrides it, and the override is recorded.
5. **Clearing on the data does not resolve an episode.** A later OK run sets
   `closed_nav_date`; the workflow state moves only when a person signs off.
   A passive breach that self-corrected still needs a decision recorded
   against it.
6. **Runs and results are immutable.** Nothing edits a recorded run. Lowering
   a limit tomorrow cannot rewrite what a run said yesterday.
7. **`Domain::Settings` gates the register.** Per the decision taken on this
   design. See "Known limitation".

## Coverage

Every check that has a defined limit and already produces a verdict:

| `check_key` | Source | Limit | Subject of a row |
| --- | --- | --- | --- |
| `issuer_10` | `analytics::concentration` | 10% NAV | issuer group |
| `forty` | `analytics::concentration` | 40% NAV | the 5%+ aggregate |
| `group_20` | `analytics::concentration` | 20% NAV | connected group |
| `fund_20` | `analytics::concentration` | 20% NAV | target fund |
| `deposit_20` | `analytics::concentration` | 20% NAV | bank |
| `liq_top5`, `liq_fixed`, `liq_hybrid_top5`, `liq_hybrid_fixed` | `handlers::limits::liquidity_h` | scenario is covered within the horizon | the scenario |
| `var_limit` | `analytics::var` + settings | `settings.var_limit` | the fund |
| `emir_<class>` (5) | `analytics::emir` | per-class threshold, WATCH at 80% | the asset class |

Rates/DV01 is excluded: it has no limit, only a disclosed sensitivity.

## The system context

The register is the fund's compliance record, not a transcript of what one
user could see. If an Operations principal without reference access triggers
an import, the recorded run must not be computed on fallback issuer groups and
liquidity defaults and then stored as evidence — that is finding P3 again, in
persisted form and harder to notice.

Register runs therefore execute under a **system context**: an `AuthCtx` with
full access, constructed only by the recorder, exactly as desktop mode's
principal is. The run row carries `inputs_complete`, false when an input was
genuinely absent (no shareholder register loaded, no CTD analytics for the
date) rather than denied — a distinction the register keeps for the same
reason the UI does.

Reading the register is grant-gated normally. Only its *computation* is
privileged, and it writes nothing a user could not have computed themselves
with full grants.

## Data model

Four tables, migration `0016_breach_register.sql`.

**`limit_check_runs`** — one per (portfolio, nav_date, trigger).

| Column | Notes |
| --- | --- |
| `id` | |
| `portfolio_id` | FK, cascade |
| `nav_date` | the snapshot the checks were struck on |
| `run_at` | wall clock |
| `triggered_by` | `import` \| `manual` |
| `import_id` | FK nullable — set when `triggered_by = 'import'` |
| `actor_user_id` | FK nullable — who triggered it; NULL in desktop mode |
| `inputs_complete` | false when an input was absent |
| `input_notes` | JSONB: what was missing, per input |

Unique on `(portfolio_id, nav_date, run_at)`. Re-importing the same workbook
is already a no-op (`duplicate: true`), so it writes no run.

**`limit_check_results`** — one per check within a run.

| Column | Notes |
| --- | --- |
| `run_id` | FK, cascade |
| `check_key` | from the coverage table |
| `scope_label` | the human phrasing already carried by `Check` |
| `limit_value`, `observed_value` | Both nullable, both in the check's own units, so the register can render "10.6% against a 10% limit" without re-deriving it. `observed_value` is the worst row's value. A check with no natural scalar pair (the liquidity scenarios, whose verdict comes from a waterfall rather than a threshold) stores NULL for both and is rendered from `status` and `detail`. |
| `status` | `ok` \| `watch` \| `breach` |
| `detail` | JSONB — the rows/waterfall behind the verdict |

**`limit_breaches`** — one per episode.

| Column | Notes |
| --- | --- |
| `portfolio_id`, `check_key`, `subject` | `subject` is the breaching row's name; the check itself where a check has no rows |
| `opened_run_id`, `opened_nav_date`, `opened_value` | |
| `peak_value`, `peak_nav_date` | worst point of the episode |
| `closed_run_id`, `closed_nav_date` | set by the first run finding the check OK for this subject; NULL while live |
| `state` | `open` \| `acknowledged` \| `resolved` |
| `classification` | `unclassified` \| `active` \| `passive` |
| `proposed_classification`, `proposal_reason` | what the machine suggested and why; kept even after an override |
| `acknowledged_by`, `acknowledged_at`, `acknowledgement_note`, `deadline_date` | |
| `resolved_by`, `resolved_at`, `resolution_note` | |

Unique on `(portfolio_id, check_key, subject)` **where `closed_nav_date IS NULL`
and `state <> 'resolved'`** — a partial index over episodes that are still in
breach on the data. At most one of those exists per subject at a time.

Note what the index deliberately allows: an episode that has cleared on the
data but is still awaiting sign-off does not block a new one. If the same
issuer breaches again next week, that is a second episode and a second thing
to explain — the first does not absorb it just because nobody has signed the
first off yet.

**`limit_breach_events`** — append-only timeline.

`(breach_id, at, actor_user_id, actor_label, event, detail)` with `event` in
`opened | classified | acknowledged | note | cleared | resolved | reopened`.
This is the evidence a reviewer reads; `audit_events` continues to record the
same acts at instance level for the administration log.

## Active/passive proposal

On opening an episode for subject S at nav_date T, `analytics::breach` compares
S's constituent instruments at T against the previous snapshot:

- **no instrument's quantity increased** → propose `passive`, reason
  `"no purchase in {S} between {T-1} and {T}; weight moved from 9.4% to 10.6%"`.
- **any increased** → propose `active`, reason
  `"quantity of {ISIN} rose from {a} to {b} between {T-1} and {T}"`.
- **no previous snapshot** → propose nothing, reason
  `"first snapshot for this portfolio; no prior position to compare"`.

Deliberately derived from positions, not the trade journal: CACEIS-fed
portfolios have no journal at all (`JOUROPLUX` is not yet parsed), and a
classification that silently skips those funds is worse than one that works
everywhere and asks for confirmation.

Liquidity, VaR and EMIR episodes have no issuer subject, so no proposal is
made and `proposal_reason` states that.

## Workflow

```
                    ┌──────────────── reopened (new episode) ──────┐
                    │                                              │
open ──acknowledge──> acknowledged ──resolve──> resolved ──breach again──┘
  │                        │
  └── a later OK run sets closed_nav_date on either state; the state does not move
```

- **Acknowledge** requires a confirmed `classification` (`active` or
  `passive`) and a non-empty note; `deadline_date` is optional.
- **Resolve** requires a non-empty note. Permitted from `acknowledged` only —
  resolving something nobody has classified is the gap this whole feature
  exists to close. Rejected from `open` with 422.
- The register shows a cleared-but-unsigned episode as
  *"cleared on the data since 14 Aug — awaiting sign-off"*, distinct from
  *resolved*.

## API

All under `/api/portfolios/{id}`.

| Route | Method | Gate |
| --- | --- | --- |
| `/limit-runs` | GET | Settings/View |
| `/limit-runs` | POST | Settings/Configure — manual re-run |
| `/breaches` | GET (`?state=`) | Settings/View |
| `/breaches/{bid}` | GET — episode + timeline | Settings/View |
| `/breaches/{bid}/acknowledge` | POST | Settings/Configure |
| `/breaches/{bid}/resolve` | POST | Settings/Configure |
| `/breaches/export` | GET — xlsx | Settings/Export |

Every route is added to `api_authz_matrix.rs`'s table in the same commit that
adds it. The export mirrors `emir::export`: one file, one sheet per section,
audited as `export`.

## UI

A new **Breaches** tab, gated on `settings/view` in `src/nav.ts`.

1. **Open episodes**, ranked breach-before-watch then by days open. Each row:
   check and subject, opened date, days open, opened → peak → current value
   against the limit, classification chip, state chip, and the action that is
   next (Acknowledge / Resolve).
2. **Run history grid** — one column per run date, one row per check, cells
   coloured by status, `inputs_complete = false` marked. This is the
   "we checked, and here is what it said" evidence, and it is the part an
   inspection actually wants.
3. **Episode detail** — the timeline, the proposal and its reasoning, and the
   notes.

Denial rendering follows the existing contract: a denied read is
`<Unavailable/>`, never an empty register, since "no breaches" and "not
permitted to see the breaches" must not look alike.

## Testing

- **analytics** (pure): episode opening/closing over a synthetic sequence of
  run results — first breach opens, persistence does not re-open, OK closes,
  a later breach opens a second episode. Active/passive proposal over
  position pairs, including the no-prior-snapshot case.
- **db**: run/result/episode persistence; the partial unique index actually
  prevents a second live episode; re-import of an identical workbook records
  no second run.
- **server**: the new routes in the authorization matrix; the state machine's
  refusals (resolve from `open`, acknowledge without a classification,
  acknowledge twice); and a run triggered by a principal without Reference
  access still records `inputs_complete = true` with correct issuer grouping —
  the system-context rule, which is the subtlest thing here.
- **frontend**: an open episode, a cleared-awaiting-sign-off episode and a
  resolved one render distinguishably; a denied register renders
  `<Unavailable/>` rather than an empty list.

## Known limitation

Gating on `Domain::Settings` means whoever may change a fund's VaR limit may
also sign off a breach against it. This was chosen deliberately over a
dedicated `Compliance` domain, to avoid a third domain so soon after the P10
split.

Mitigations: every acknowledgement and resolution is attributed, timelined and
audited; runs are immutable, so a limit lowered after the fact cannot rewrite
what a past run recorded. If the separation is wanted later it is one domain
and one migration — the same shape as `0015_settings_domain.sql`.

## Out of scope

- **Alerting.** Notification on a new breach is roadmap item 6 and needs a
  delivery channel decision first.
- **Scheduled runs.** There is no scheduler in this process; import-triggered
  plus manual is the whole trigger set.
- **Deadline enforcement.** `deadline_date` is recorded and shown as overdue;
  nothing escalates on it.
- **Retrospective backfill.** The register starts from the first run after
  deployment. Backfilling historical snapshots is a separate decision, since
  every backfilled run would be computed with today's reference data rather
  than the data as it stood.
