# P&L

The P&L tab shows how much money the portfolio made or lost over a chosen period, breaks that
figure down by instrument and by grouping (asset class, country, sector, and so on), splits each
figure into price and currency effects, and reconciles the total back to the fund's own reported
change in assets under management (AUM). It is the page to use when someone asks "what drove the
performance this month" or "does our P&L tie out to the administrator's numbers."

## Choosing a period

At the top of the page is a row of controls:

- **Period presets** — four buttons: **MTD** (month to date), **QTD** (quarter to date), **YTD**
  (year to date, from 1 January of the current year), and **ITD** (inception to date, effectively
  "from the earliest data available"). Clicking one sets the date range instantly and reloads the
  table below.
- **Custom date range** — two date pickers ("from" and "to") next to the presets. Pick any two
  dates directly if none of the presets fit. The "from" date cannot be after the "to" date.
- **Group by** — a dropdown that controls how the instrument-level figures are aggregated in the
  table (see "Grouping dimensions" below).

### Dates are struck, not interpolated

The dates you pick are a request, not a guarantee. The page can only compute P&L between two dates
on which a position snapshot was actually imported into the system — it never estimates or
interpolates a value for a date that wasn't imported. So for each end of your requested range, the
page uses the nearest imported snapshot on or before that date (falling back to the earliest
available snapshot if none qualifies).

Whenever the dates actually used differ from what you asked for, a note appears just below the
controls, for example:

> Struck between imported NAV dates 2026-06-28 and 2026-08-15 (12 snapshots). You asked for
> 2026-06-30 → 2026-08-19.

The snapshot count in parentheses is how many imported dates fall within the struck range
(inclusive), which gives a rough sense of how much data underlies the figures.

If fewer than two position snapshots have ever been imported for the portfolio, the table cannot
be built at all and the page shows an explanatory message instead. Similarly, if your requested
range collapses onto a single snapshot date (for example both ends of the range snap to the same
imported date), the page reports that instead of showing an empty or misleading table.

## Missing classification warning

Next to the controls, a badge reading "*N* instruments missing classification data" appears
whenever one or more instruments in the portfolio have no country **and** no sector information at
all. This count does not change when you switch the "Group by" dimension — it always reflects the
same underlying data gap, regardless of which dimension the table is currently grouped by. See
[Reference data](data.md) for how classification data is entered or uploaded, and where it comes
from.

## The P&L table

The main table lists one row per group (per the selected "Group by" dimension), each of which can
be expanded to show the individual instruments inside it. Columns:

| Column | Meaning |
|---|---|
| Group | The group's name (or an instrument's name, once expanded), with a ▸/▾ arrow to expand or collapse the group. |
| Realized | The portion of P&L that has been locked in by an actual trade (a sale, or — for futures — a closed contract), combining both price and currency effects. |
| Unrealized | The portion of P&L still sitting in open positions (mark-to-market movement since the period start), combining both price and currency effects. |
| of which FX | The currency-translation component of the total, whether realized or unrealized. |
| Total | Realized + unrealized, the group's or instrument's full period P&L in euros. |

Click anywhere on a group row to expand it and see the instruments inside, indented, each showing
its own realized/unrealized/FX/total split. Groups and instruments within a group are both sorted
by the size of their total P&L (biggest movers first, whether gains or losses). A **Total** row at
the bottom sums every group's total into the portfolio's overall period P&L.

### How the price/FX/realized/unrealized split works

For each instrument, the page separates the period's local-currency price movement from the
currency-translation effect, and separates both into a realized piece (locked in by an actual
sale) and an unrealized piece (still mark-to-market). The realized/unrealized split for price
uses **weighted-average cost**: every purchase updates a running average cost, and a sale realizes
the difference between the sale price and that average cost at the time of the sale.

Two things worth knowing about this method:

- **Futures have no cost basis.** A future is not bought at a price that can be averaged the way a
  bond or equity is — it has no acquisition cost. Its entire P&L is instead the change in
  variation margin over the period, always reported as unrealized. Consequently, when a bond future
  position is closed out during the period, its realized result is **not yet computed** from the
  trade history and is reported as zero; the true realized gain or loss on that closed future ends
  up as an unexplained amount in the reconciliation's residual line (see below) rather than being
  attributed to the instrument.
- **A partial sale after a mid-period purchase cannot split currency effects exactly.** When a sale
  draws on a cost basis that includes a purchase made earlier in the same period, weighted-average
  costing cannot say precisely how much of that purchase's currency movement belongs to the shares
  sold versus the shares still held. In this situation the instrument's name in the table carries a
  **⚠ warning icon** (hover it for the explanation). The total P&L for the instrument is still
  correct — only the realized/unrealized split of its FX component is an approximation in this
  specific case. A sale that only draws on the position held at the start of the period (i.e. it
  precedes every purchase in the period) is never flagged, because there is no ambiguity to split.

### Portfolios fed from CACEIS have no trade-level attribution

A portfolio whose positions are imported from the CACEIS CSV feed does not currently have a trade
journal loaded into the system. Without trade history, the page cannot separate realized from
unrealized P&L or attribute currency effects the way it does for a portfolio with a full journal —
every instrument in such a portfolio is treated as having no trades, so its entire period movement
shows as unrealized price/FX change, with no realized figure. See
[Data import](data.md) for more on what each import source provides.

## Grouping dimensions

The "Group by" dropdown offers seven ways to slice the table:

- **Asset class** — Equities, Bonds, Funds, Futures, and similar broad categories, derived
  automatically from each position's instrument type.
- **Country** — the instrument's country of risk.
- **Region** — the instrument's geographic region.
- **Sector** — the instrument's GICS sector.
- **Industry** — the instrument's GICS industry.
- **Currency** — the instrument's trading/settlement currency.
- **Issuer group** — the parent issuer group the instrument has been assigned to (used, among other
  things, for concentration monitoring).

Country, region, sector, industry and issuer group all come from the portfolio's reference data —
entered by hand, uploaded from a Bloomberg export, or, for a CACEIS-fed portfolio, pre-filled
automatically on import where those fields were still empty. See [Reference data](data.md) for how
to view or edit them.

Any instrument missing a value for the selected grouping dimension is placed in an **Unclassified**
bucket within the table, so it is never silently dropped from the total — only regrouped into a
catch-all.

## Reconciliation

Below the main table, a **Reconciliation** card ties the P&L figures back to the fund's own
reported change in AUM, so you can see at a glance whether the numbers are internally consistent.
The rows are:

| Row | Meaning |
|---|---|
| Investment P&L | The sum of every instrument's period P&L (the same figure the table above breaks down). |
| Cash and margin accounts | The net movement in cash and margin balances that is not already explained by trade settlements, subscriptions/redemptions, or dividend receipts — in practice this is mostly FX revaluation of cash balances, interest, and any other unexplained cash movement. |
| Accrued fees | The change in accrued-fee provisions over the period. |
| Provisions | The change in order-related provisions over the period. |
| Dividend income | Dividends accrued (not necessarily received in cash) during the period, converted to euros at the rate on their accrual date. |
| **Total P&L** | The sum of the five lines above. |
| AUM change | The fund's actual change in assets under management over the struck period, taken from the imported NAV history. |
| less subscriptions / redemptions | Net investor flows (subscriptions minus redemptions) over the period, inferred from the day-to-day change in shares in issue priced at each day's NAV — the system does not currently receive subscription/redemption events directly. |
| Residual | AUM change, minus flows, minus Total P&L. In principle this should be zero; in practice it captures whatever the P&L lines above could not explain. |

The residual is always shown, never hidden. When it is small relative to the total activity in the
period, it is shown in the normal (positive) color with the word **"reconciled"**; once it exceeds
tolerance it switches to the negative/warning color and reads **"above tolerance"**, together with
its size as a percentage of gross P&L (the sum of the absolute value of each of the five P&L
lines). A period with no P&L activity at all still requires the residual to be very close to zero
in absolute euro terms to read as reconciled — an unexplained AUM movement is always flagged as a
breach, never silently passed, even when there is nothing to compare it against proportionally.

A non-trivial residual is not necessarily an error: it is exactly where the effects the page cannot
otherwise attribute end up, most notably a bond future's realized P&L on a mid-period close (see
above), or any other trade/FX data gap. Investigate the residual by first checking the warnings
described below.

## Warnings

Below the reconciliation, the page lists any data-quality warnings for the period, each as a small
badge. These are informational — they explain gaps or approximations in the current figures without
stopping the page from showing what it can compute. Warnings that can appear include:

- **Trade history denied or unavailable** — if you do not have access to transaction data for this
  portfolio, trade-level P&L attribution cannot be computed and the reconciliation's residual may be
  distorted. See [Access rights](access-rights.md).
- **Unrecognized trade side** — a trade record had a side value that wasn't recognized as a buy or
  sell, so it was ignored.
- **Sells exceed recorded buys** — the trade history for an instrument shows more shares sold than
  were ever bought (an "oversold" condition), meaning the figures for that instrument are
  incomplete.
- **No FX rate for a trade or dividend** — a currency had no exchange rate available for a specific
  trade date or dividend accrual date, so that flow was excluded from the P&L rather than guessed.

## Access rights

The P&L tab requires permission to view portfolio positions; without it, the whole page shows an
access-denied message in place of the controls and table. Several other pieces of data feed into
the page — transactions/trade history, reference data (classifications), market data (FX rates) and
NAV history — and each of these degrades independently and gracefully rather than blocking the
page: if you lack access to one of them, the page still renders with whatever it can compute from
the rest, and calls out the gap through the warnings described above (most visibly for trade
history, since it drives the realized/unrealized split). See [Access rights](access-rights.md) for
details on how permissions are structured.
