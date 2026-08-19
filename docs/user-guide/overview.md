# Overview

The Overview tab is the landing page for whichever portfolio is currently
selected in the navigation. It gives a one-screen summary of where the fund
stands today: its latest NAV and assets, its return so far this year, its
volatility and risk-adjusted return, its worst historical decline, and its
regulatory VaR usage — followed by two charts covering the fund's full
history.

Everything on this page is scoped to the selected portfolio only; switching
portfolios in the navigation reloads the whole page with that portfolio's own
figures.

## When there is no data yet

If nothing has been imported for the portfolio, the page shows a single
notice — "No data yet" — with a link to the Data page to import a NAV Recap
file. None of the cards or charts are drawn in this state.

## When you don't have access

If you are not authorized to view this portfolio's NAV history, the whole
page is replaced by an "N/A" notice instead of the summary — see
[Access rights](access-rights.md) for what determines this and how to
request access.

## Data-quality warnings

Just under the page title, small warning badges can appear above the KPI
cards. They call out cases where a figure is not computed on the intended
basis because the fund's history is still short, for example:

- "Metrics n/a: only *N* observations (< 30)" — fewer than 30 days of
  returns are available, so every KPI on this page (other than NAV, AUM and
  the "as of" date) shows a dash.
- "1Y metrics use full available history (*N* obs < 252)" — there is enough
  history for the metrics but less than a full year, so the "1Y" figures are
  computed over all the history available rather than a true trailing year.
- "VaR n/a: only *N* observations (< 30)" or "VaR window shrunk to available
  history (*N* obs < *window*)" — the same shortage, specific to the VaR
  card.

## KPI cards

The top row shows seven cards. A dash ("–") in any card means the figure
could not be computed (not enough history, or, for the VaR card, an invalid
confidence level).

| Card | What it shows |
| --- | --- |
| **NAV** | The fund's net asset value per share/unit as of the latest imported date, shown as a plain number. |
| **AUM** | The fund's total assets under management (net assets) as of that same date, shown in euros. |
| **YTD** | Year-to-date return: the latest NAV divided by the NAV on the last available date of the prior calendar year, minus one. If there is no prior-year data yet (a fund still in its first calendar year), it falls back to the return since inception. |
| **Vol 1Y** | Annualized volatility of daily returns over the trailing year (up to 252 trading days of history; fewer if the fund is younger — see the warning badges above), computed as the sample standard deviation of daily returns scaled by the square root of 252 trading days. The sub-line, **Inception**, shows the same annualized-volatility calculation using the full return history since inception instead of just the trailing year. |
| **Sharpe 1Y** | The Sharpe ratio over the trailing year: the annualized return over that period minus the portfolio's risk-free rate, divided by the annualized volatility over the same period. The risk-free rate is a per-portfolio setting (2% per year by default). The sub-line, **Yield/Vol**, shows the same ratio without subtracting the risk-free rate — annualized return divided by annualized volatility. |
| **Max drawdown** | The deepest peak-to-trough decline in NAV over the fund's entire history: at every date, NAV is compared against the highest NAV reached up to that point, and this card shows the worst (most negative) of those readings since inception. This never resets — contrast with the per-year figures on the [Performance](performance.md) tab, which do reset each calendar year. |
| **VaR *c*%/*h*d** | Value-at-Risk at the portfolio's configured confidence level and horizon (99% confidence over a 20-trading-day horizon by default), computed by the historical method over the configured trailing window (252 trading days by default) and scaled to the horizon. The sub-line shows the portfolio's VaR **limit** (20% of NAV by default) and how much of that limit is currently **used** (this VaR divided by the limit). All of these — confidence, horizon, window and limit — are portfolio settings and can differ by portfolio. A full breakdown across all three VaR methods, plus back-testing, is on the Risk / VaR pages reachable from elsewhere in the app. |

## NAV chart

A line chart of the fund's NAV per share/unit over its full imported
history, one point per imported date. The x-axis is the date; the y-axis is
the NAV level, auto-scaled to the data (it does not necessarily start at
zero). Hovering shows the exact NAV for a date. A zoom slider below the
chart, plus scroll-to-zoom on the chart itself, lets you focus on a shorter
period.

## Drawdown chart

An area chart of the fund's underwater curve since inception: at each date,
how far NAV sits below its running historical peak, as a percentage (always
zero or negative). The x-axis is the date; the y-axis is the percentage
decline. This is the same peak-to-trough calculation that feeds the **Max
drawdown** KPI card above — the lowest point on this chart is that card's
value. For a table of the worst drawdown per calendar year, and the list of
the deepest *short*-duration drawdowns, see the [Performance](performance.md)
and [Risk](risk.md) tabs.
