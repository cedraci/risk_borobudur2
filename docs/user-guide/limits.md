# Limits

The Limits tab is the fund's compliance and liquidity cockpit. It runs UCITS
concentration checks, models how quickly the fund could meet a redemption
under normal and stressed conditions, and lays out interest-rate exposure
from bonds and bond futures. Everything on the page is computed from the
position snapshot you select at the top — there is nothing to save here, it
is a read-only, point-in-time view.

All three panels (Concentration, Liquidity, Rates) read from the same
snapshot. If you cannot view positions for the selected portfolio at all, the
whole page shows as unavailable; see [Access rights](access-rights.md).
Several panels also draw on reference data, NAV history, or the shareholder
register, each governed by its own access grant — where one of those is
missing, the affected figures degrade to an explicit unavailable state rather
than silently showing an incomplete result. Those cases are called out in
each section below.

## Snapshot selector

A single dropdown, labelled **Snapshot**, lists every date for which the
portfolio has a position snapshot, most recent first. Selecting a date
reruns all three panels (Concentration, Liquidity, Rates) against that
snapshot. Leaving it on the default value uses the most recent snapshot
available.

## Concentration

This panel runs the fund's UCITS concentration checks against the selected
snapshot. Five checks are shown, each as its own card with a heading (the
scope of the check), a status chip, and a table.

### The five checks

| Check | Scope | Limit | Watch from |
|---|---|---|---|
| Issuer ≤ 10% NAV | Each issuer group among equities and bonds (plus dividend receivables) | 10% of NAV | 8% |
| Sum of issuers > 5% ≤ 40% NAV | The combined weight of every issuer group above individually exceeding 5% of NAV | 40% of NAV | 32% |
| Connected group ≤ 20% NAV | The same issuer groups as the issuer check, against a wider limit | 20% of NAV | 16% |
| Target fund ≤ 20% NAV | Each fund (UCI) the portfolio holds, grouped by fund name | 20% of NAV | 16% |
| Deposits per bank ≤ 20% NAV | Each bank, across cash and margin accounts | 20% of NAV | 16% |

Notes on how these are built:

- The "issuer ≤ 10%" and "connected group ≤ 20%" checks are computed from
  exactly the same per-issuer-group weights — they differ only in the limit
  applied, so the two tables will show identical groups and weights, just
  measured against 10% and 20% respectively.
- The "sum of issuers > 5%" check adds together only the issuer groups that
  individually exceed 5% of NAV, and compares that single total against 40%.
- Equity, bond and dividend-receivable positions are grouped by issuer. By
  default the group is the position's name, normalised to uppercase; cash
  and margin accounts are grouped by the bank code after the last "- " in
  the account name (e.g. "Depositary Bk- CBLU" groups as "CBLU"). This can
  be overridden per instrument on the Data page — see
  [Data](data.md#reference-data) — except for target-fund (UCI) holdings,
  which are always grouped by their own fund name regardless of any
  issuer-group override.
- The target-fund check only looks at positions of type "Fonds"; fund
  holdings are excluded from the connected-group check so they are not
  counted twice.
- Deposits are netted within each bank (a negative margin balance offsets a
  positive cash balance in the same group); a bank whose net balance is zero
  or negative is dropped from the table rather than shown as a negative row.
- A footnote under the checks states: futures are excluded from the issuer
  limits (their notional is not an issuer exposure under the 5/10/40 rule),
  and fee and order-provision positions are excluded entirely.

### Status thresholds

Every row and every card carries one of three statuses:

- **OK** — weight below 80% of the limit.
- **WATCH** — weight at or above 80% of the limit, but not over it.
- **BREACH** — weight over the limit.

A check's card-level status is the worst status among its rows. A check with
no positions in scope shows "No positions in scope." instead of a table.

### When it is unavailable

The issuer-group overrides and the target-fund grouping described above
depend on reference data. If your access does not include reference data for
this portfolio, an unavailable banner appears above the checks, and **every**
check and every row inside it shows as unavailable (grey "N/A") rather than
as a computed OK — a real breach must never be hidden behind a check that
still looks like it passed just because the overrides could not be applied.
Hover the unavailable badge to see the reason. See
[Access rights](access-rights.md).

## Liquidity

This panel estimates how long it would take the fund to raise cash to meet a
redemption, under the fund's own configured assumptions, and shows the same
information visually as a liquidity profile by day-band.

### Parameters

A line above the chart states the current settings, all configured on the
portfolio's settings screen:

- **Participation rate** — the share of a position's 30-day average daily
  volume (ADV) the fund assumes it can trade per day in the normal case.
- **Stress factor** — the multiplier applied to ADV in the stressed case
  (smaller than the participation rate, since a stress event thins trading).
- **Horizon** — the number of business days modelled (Monday–Friday, no
  holiday calendar).
- **Settlement deadline** — the number of business days within which the
  required cash must be available for a scenario to read OK.

### Coverage and data-quality notes

- **ADV measured on X% of NAV** — the share of the fund's positive-value
  positions for which capacity was actually measured from traded volume,
  rather than assumed. The remainder are listed as "position(s) on the
  assumed-days fallback", expandable to see each instrument's code and the
  reason: `future` (futures cannot be measured this way), `not eligible`
  (the position's venue is not a real trading market, or an override marks
  it ineligible), `stale adv` (the volume figure is older than the
  configured maximum age), `no adv` (no volume figure at all, or the figure
  computes to a non-positive capacity), or `no quantity`.
- A position on the assumed-days fallback uses either a per-instrument
  liquidity-days override or the asset-type default — both configured on the
  Data page, see [Data](data.md#liquidity-defaults). The stress factor is
  **not** applied to fallback positions (an assumption is not re-stressed),
  so the stressed profile only moves the ADV-measured share of the fund; the
  page states this explicitly, quoting the same coverage percentage.
- **Bond coupon/redemption gaps** — bonds contributing no coupon inflow to
  the cash-raising calculation, expandable to see each bond's code and
  reason: `no next coupon date`, `no resolvable frequency` (the payment
  frequency could not be read from the depositary feed or inferred from
  accrued interest), or `no fx rate` (the position's local-currency market
  value is missing, so its coupon cannot be safely converted to EUR). A
  zero-coupon bond is not reported as a gap — it simply pays nothing. A
  bond's face value redeeming at maturity within the horizon is credited as
  its own inflow, separate from and in addition to any coupon inside the
  same horizon.
- If the shareholder register is more than 90 days old, a warning states
  "Shareholder register is stale (as of \<date\>)."
- If your access does not include the shareholder register, this line shows a
  grey N/A naming the missing permission instead. A denied register otherwise
  looks exactly like an empty one — no holders, nothing stale — so the notice
  is the only thing that tells the two apart here.

### Liquidity profile chart

A bar-and-line chart shows the fund's positive-value holdings split across
four day-bands, as a percentage of NAV:

- **1 day**
- **2–7 days**
- **8–30 days**
- **> 30 days**

A position's band is set by its days-to-liquidate: measured positions use
value ÷ (ADV × participation rate × stress factor); fallback positions use
their configured days figure. Bars are shown for both the **Normal** and
**Stressed** cases, with matching cumulative lines. Negative positions
(payables, short cash) are excluded from this chart and reported separately
below. If the snapshot holds no positions, the chart is replaced with "No
positions in this snapshot."

### Redemption scenarios

A table lists four scenarios, each row clickable to plot its cash-raising
curve below:

| Scenario | Required redemption | ADV assumption |
|---|---|---|
| Top 5 holders | Sum of the top 5 shareholders' percentage of NAV, from the shareholder register | Normal |
| Fixed shock | The portfolio's configured redemption shock | Normal |
| Top 5 holders, stressed ADV | Same as Top 5 holders | Stressed |
| Fixed shock, stressed ADV | Same as Fixed shock | Stressed |

Columns: **Scenario**, **Required** (EUR), **Waterfall days** (the first
business day on which raised cash plus inflows meets the requirement, or
"> horizon" if it never does within the modelled horizon), **Slice days**
(a day figure assuming every position is sold in the same proportion it is
held, always slower than or equal to the waterfall — a measure of how long
a strictly pro-rata liquidation would take), **Unmet** (the shortfall in EUR
if the horizon is exhausted before the requirement is met), and **Status**.

Cash raised is modelled by selling the most liquid positions first
("waterfall"); coupon and redemption inflows land on their own business day,
not before; and any negative positions (payables, short cash) reduce the
cash available from day one rather than being netted in later.

- **Status is OK** when the waterfall completes within the settlement
  deadline (days ≤ the configured deadline shown in the parameters line).
- **Status is BREACH** otherwise — including when the horizon is exhausted
  with cash still unmet.
- **Status is unavailable** for the two "Top 5 holders" scenarios when no
  shareholder register is loaded for this portfolio, or when your access
  does not include the shareholder register at all (the reason shown
  distinguishes "not permitted to view the register" from "no register has
  been loaded yet" — see [Access rights](access-rights.md)). The Fixed
  shock scenarios are never unavailable this way, since the redemption
  shock is always a configured value.

Selecting a row (default: Fixed shock) draws a line chart of cash available
by day, with dashed reference lines marking the required amount and the
settlement deadline.

### Observed flow history

Below the scenario table, a note compares the configured redemption shock
against the fund's own recent subscription/redemption history (loaded from
the depositary's daily flow file):

- With at least 20 loaded daily observations, it shows the worst observed
  20-business-day net outflow as a percentage of NAV, the number of
  observations and their date range, and restates the configured shock for
  comparison. Any date excluded because a share class's NAV had not yet
  been struck that day is counted and noted.
- With fewer than 20 observations, the note reads unavailable and states how
  many observations are loaded against the 20 required, with a prompt to
  load JOURSRLUX files on the Data page. This observed history only
  **informs** the judgement of whether the configured shock is reasonable —
  it never sets or overrides the shock itself.

### Negative positions

A final line states the fund's negative positions (payables, short cash) as
both a percentage of NAV and a EUR amount, noting that they reduce
availability from day one and are not netted anywhere else in the model.

### When liquidity is unavailable or degraded

- If your access does not include reference data, an unavailable banner
  appears at the top of the panel. Because every per-instrument ADV
  eligibility flag, liquidity-days override, and issuer-group enrichment
  comes from reference data, **every** redemption scenario (Top 5 and Fixed,
  normal and stressed alike) is also forced to unavailable, along with its
  waterfall, slice-days and chart curve — not left to display a value
  computed on unverified fallback assumptions. See
  [Access rights](access-rights.md).
- If your access does not include NAV history for this portfolio, or the
  snapshot date has no NAV recorded, an unavailable banner names the reason.
  Since the requirement in EUR for every scenario depends on NAV, the panel
  falls back to its empty state ("No positions in this snapshot.") with no
  coverage chart and no scenarios, rather than showing figures computed
  against an unknown NAV.

## Rates

This panel shows bond-level yield and duration analytics, the portfolio's
aggregate DV01 (euro sensitivity per basis point), and its NAV sensitivity
to a parallel 100bp rate move — covering both cash bonds and bond futures.

### Bonds table

One row per bond position in the snapshot, with columns **Bond**, **Coupon**,
**Maturity**, **Price**, **YTM**, **Mod. duration**, **DV01 €**, and
**Weight**.

- YTM and modified duration are computed from the clean price, the fixed
  coupon rate, the coupon frequency, and the maturity date, using standard
  bond math (semi-annual or annual coupons only). DV01 is the modified
  duration times the bond's market value, scaled to a one-basis-point move.
- A bond whose coupon rate, maturity, or coupon frequency cannot be
  resolved from reference data shows "missing reference data" instead of
  figures, and is excluded from the portfolio DV01 total. If any bond is
  missing this way (and reference data itself is accessible), a warning
  above the table prompts filling in coupon/maturity/frequency on the Data
  page.
- Only bonds with a fixed coupon type are analysed at all; floating-rate or
  other coupon types are treated the same as missing reference data.

### Portfolio DV01 and NAV sensitivity

Below the table: **Portfolio DV01** (the sum of DV01 across all bonds and
bond futures, in EUR) and **NAV sensitivity per +100bp**.

The sensitivity is signed as profit and loss, not shown as a magnitude: it
equals −100 × total DV01 ÷ AUM at the snapshot date. A **negative** value
means net assets fall if yields rise 100bp (the book is net long rates); a
**positive** value means net assets rise (the book is net short rates once
futures are included). It is shown as "–" whenever the snapshot's AUM is
unknown, and also when reference data could not be checked at all (see
below) — a page note states this explicitly.

### Bond futures

A second table lists interest-rate bond futures, with columns **Future**,
**Qty**, **Price**, **Point value**, **CTD ISIN**, **Mod. duration**,
**Conv. factor**, and **DV01 €**. DV01 for a future is derived from that
future's cheapest-to-deliver (CTD) analytics — modified duration, clean
price, accrued interest and conversion factor — supplied through a weekly
companion file uploaded on the Data page. **CTD analytics are never carried
forward from a previous week**: if the snapshot's NAV date has no matching
CTD upload, that future's DV01 is reported as missing for this date, shown
in the table as "missing CTD analytics for this date" and excluded from the
portfolio DV01 total.

Two warnings can appear above this table:

- A future held with no contract spec at all (its root ticker is not known
  to the system) is excluded from DV01 and listed by name, with a prompt to
  re-upload the NAV Recap on the Data page to seed its spec and then confirm
  it.
- If any bond future is missing CTD analytics for this snapshot date, a
  warning prompts uploading the weekly CTD companion file on the Data page.

If there are no interest-rate futures in the snapshot at all, the table is
replaced with "No interest-rate futures in this snapshot." — but if futures
are held and simply could not be evaluated (no spec), the message instead
reads "No bond-future DV01 could be computed for this snapshot", so absence
of a table is never confused with absence of holdings.

### When rates is unavailable or degraded

If your access does not include reference data, an unavailable banner
appears at the top of the panel. Every bond then shows as unavailable rather
than "missing reference data" (the two are visually and semantically
distinct: the first means "could not check", the second means "checked, and
the data is genuinely absent"). Bond futures cannot be classified as
interest-rate contracts without reference data either, so they lose their
spec and are reported as held with no contract spec at all. Portfolio DV01
and NAV sensitivity are shown as "–" rather than as a computed (and
misleadingly confident) zero. See [Access rights](access-rights.md).

Unlike the Liquidity panel, the Rates panel does not display a separate
banner when your access excludes NAV history — a denied or genuinely unknown
AUM both simply show the NAV-sensitivity figure as "–", with no way to tell
the two apart from this panel alone.
