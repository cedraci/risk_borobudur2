# Performance

The Performance tab breaks the selected portfolio's return down by calendar
period — month, quarter and year — and shows how deep its worst decline was
in each calendar year. It has no controls: it always shows the full history
available for the portfolio currently selected in the navigation.

## When you don't have access

If you are not authorized to view this portfolio's NAV history, the page
shows an "N/A" notice instead of the tables below — see
[Access rights](access-rights.md).

## Monthly returns

A table of month-by-month returns for the three most recent years with data,
most recent year first. Each row is one year, with a column per calendar
month (Jan–Dec) plus a final **Year** column for that year's total return.

- Each monthly cell is the fund's return for that calendar month: the NAV on
  the last imported date of the month divided by the NAV on the last
  imported date of the previous month, minus one. For the very first month
  of the fund's history, it is measured against the first imported NAV
  instead.
- A month with no data yet (the current, still-incomplete month, or a month
  before inception) is left **blank** — not a dash, an empty cell.
- Each cell's background is tinted to show gains and losses at a glance:
  green for a positive month, red for a negative one, with the intensity of
  the tint scaling up to a 5% move (a 5%-or-larger move gets the strongest
  tint; smaller moves are paler).
- The **Year** column is the fund's compounded return for the full calendar
  year (see "Max drawdown per year" below and the [Overview](overview.md)
  tab for how this compares to the since-inception figures). It is styled
  green when positive and red when negative.

## Quarterly returns

The same idea, one row for each of the three most recent years, with columns for
Q1–Q4 instead of months. Each quarterly cell is the NAV on the last imported
date of the quarter versus the last imported date of the previous quarter
(or, for the fund's first quarter, versus its first imported NAV), with the
same blank-if-missing and green/red heat-tinted background as the monthly
table. There is no year-total column on this table (use the monthly table's
**Year** column, or the KPI cards on the [Overview](overview.md) tab, for
annual totals).

## Max drawdown per year

A table with one row per calendar year the fund has data for, plus a final
**Since inception** row.

- Each yearly row shows the deepest peak-to-trough decline in NAV observed
  *within that calendar year*: the running peak used for the calculation
  resets to the year's first NAV at the start of each year, so a decline
  that started before the year began is not counted against that year.
- The **Since inception** row shows the single deepest peak-to-trough
  decline across the fund's entire history, with the peak never resetting —
  this is the same figure as the **Max drawdown** KPI card on the
  [Overview](overview.md) tab.
- Both are shown as negative percentages.

For the *shortest*-duration drawdowns — the fastest, sharpest drops rather
than the deepest ones regardless of how long they took — see the "Top 5
drawdowns over short periods" table on the [Risk](risk.md) tab.
