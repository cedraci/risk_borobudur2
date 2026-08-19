# Risk

The Risk tab shows how the selected portfolio's volatility and risk-adjusted
return have evolved over time, using a rolling (trailing) window you choose,
plus a table of its sharpest short-lived drawdowns.

## When you don't have access

If you are not authorized to view this portfolio's NAV history, the page
shows an "N/A" notice instead of the charts and table below — see
[Access rights](access-rights.md).

## Rolling window control

A dropdown at the top lets you pick the trailing window used by the three
charts below: 20, 60, 120 or 252 trading days (60 days is selected by
default). Changing it recomputes and redraws all three charts immediately.
Each chart needs at least that many days of returns before it can plot its
first point — a portfolio younger than the selected window shows an empty
chart until enough history accumulates; switching to a shorter window is the
way to see data sooner for a young fund.

Every point on the three charts below is dated at the *last* day of its
trailing window, and each is computed independently from the daily returns
inside that window only (not from the whole history).

## Annualized volatility (rolling)

A line chart of annualized volatility, recalculated for every day using only
the trailing window of daily returns ending on that day: the sample standard
deviation of those daily returns, scaled by the square root of 252 trading
days. The x-axis is the date; the y-axis is the percentage. This is the same
calculation as the **Vol 1Y** KPI on the [Overview](overview.md) tab, but
recomputed continuously over the chosen rolling window rather than fixed to
the trailing year.

## Sharpe ratio (rolling)

A line chart of the rolling Sharpe ratio: for each day, the annualized
return over the trailing window minus the portfolio's risk-free rate, all
divided by the annualized volatility over that same window. The risk-free
rate is a per-portfolio setting (2% per year by default, editable in
Settings). The x-axis is the date; the y-axis is a plain ratio (not a
percentage). This is the rolling version of the **Sharpe 1Y** KPI on the
[Overview](overview.md) tab.

## Yield / volatility (rolling)

The same idea as the Sharpe chart, but without subtracting the risk-free
rate: rolling annualized return divided by rolling annualized volatility.
The x-axis is the date; the y-axis is a plain ratio. This is the rolling
version of the **Yield/Vol** figure shown as the Sharpe card's sub-line on
the [Overview](overview.md) tab.

## Top 5 drawdowns over short periods

A table listing the five deepest peak-to-trough NAV declines whose duration
(peak date to trough date) falls within the portfolio's configured
short-drawdown threshold — 50 calendar days by default, shown in the table's
title and editable in Settings. This is a different cut of the same
underlying drawdown history as the [Performance](performance.md) tab's
yearly table and the [Overview](overview.md) tab's Max drawdown card: those
show the single deepest decline regardless of how long it took, while this
table specifically surfaces the sharpest, fastest drops, ranked deepest
first.

Columns:

| Column | Meaning |
| --- | --- |
| **#** | Rank, 1 (deepest) to 5. |
| **Peak** | Date of the NAV high that the decline started from. |
| **Trough** | Date of the lowest NAV reached during the decline. |
| **Depth** | The decline from peak to trough, as a negative percentage. |
| **Days** | Calendar days between the peak and the trough. |
| **Recovered** | The date NAV first climbed back to or above the peak, or the word "ongoing" if it has not yet recovered as of the latest imported date. |

If no drawdown episode is short enough to qualify, the table has no rows.
