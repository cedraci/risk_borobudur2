# VaR / Expected Shortfall

The VaR / Expected Shortfall tab shows how much the portfolio could lose over a
given horizon, computed three different ways, checked against the fund's UCITS
risk limit, and validated against what actually happened using a regulatory
back-test.

Everywhere on this tab, **VaR and ES are shown as positive numbers that mean a
loss**, expressed as a percentage of NAV. A VaR of "2.30%" means: at the chosen
confidence level, the portfolio is not expected to lose more than 2.30% of its
NAV over the chosen horizon. Expected Shortfall (ES, also called CVaR) is the
average loss in the worse scenarios beyond that threshold — the answer to "if
the loss does exceed VaR, how bad is it on average?" ES is always at least as
large as VaR.

If your role does not carry the right to view this fund's NAV history, the tab
shows a single "N/A" notice instead of any figures — see
[Access rights](access-rights.md).

## The controls

At the top of the tab, four controls apply to everything shown in the top half
of the page (the method cards, the rolling chart and the limit-breach table —
not the back-test section, which is described separately below):

- **Confidence** — a dropdown with 95.0%, 97.5% or 99.0%. This is the
  statistical confidence level of the VaR/ES estimate: at 99%, the loss is
  expected to be exceeded on only 1% of days. The tab opens on the fund's
  configured monitoring level (typically 99%).
- **Horizon** — a dropdown with 1, 10 or 20 trading days. The 1-day VaR/ES
  computed from daily returns is scaled up to the chosen horizon using the
  square-root-of-time rule (the 1-day figure multiplied by the square root of
  the number of days). This is a standard approximation and assumes returns
  from one day to the next are independent and identically distributed; it
  tends to understate risk when losses cluster (e.g. in a stressed market).
  The tab opens on the fund's configured horizon (typically 20 days).
- **Window** — a number field (minimum 30) for how many trailing daily
  returns feed the calculation. The tab opens on the fund's configured window
  (typically 252 trading days, roughly one year). A longer window smooths the
  estimate over more history; a shorter one reacts faster to recent
  volatility.
- **UCITS limit** — read-only, shown next to the other controls. This is the
  fund's configured absolute VaR ceiling (typically 20% of NAV), the
  regulatory reference point the "Limit utilization" card and the rolling
  chart are measured against. The standard UCITS commitment-approach
  monitoring point is 99% confidence over a 20-trading-day horizon — if the
  confidence and horizon controls are left on their defaults, the top of the
  page is showing exactly that regulatory figure.

Changing any control instantly recomputes the method cards, the limit
utilization card and the rolling-VaR chart. It does **not** change the
back-testing section further down the page, which always runs at 1-day
horizon and 99% confidence regardless of what is selected here (see below).

If your role can view VaR but not the fund's settings, the page still works —
it silently falls back to the standard 99% / 20-day / 252-day defaults for the
top controls, but the settings themselves are unavailable and a separate
notice is shown for that (see [Access rights](access-rights.md)).

## The three VaR/ES methods

Four cards summarise the current calculation, one per method plus a limit
card:

- **Historical** — VaR and ES read directly off the sorted history of past
  daily returns over the selected window, with no assumption about their
  statistical shape. VaR is the loss at the chosen percentile of that
  historical distribution (e.g. the loss that was exceeded on only 1% of days
  in the window, at 99% confidence); ES is the average of the losses beyond
  that point. Because it uses the actual historical returns as-is, it
  naturally reflects any fat tails or skew that were present in that history.
  This card also shows the VaR converted into a euro amount, using the
  portfolio's latest AUM — this conversion is only shown for the Historical
  method, since it is the one measured against the UCITS limit.
- **Gaussian** — VaR and ES computed analytically assuming daily returns
  follow a normal (bell-curve) distribution with the window's own average
  return and volatility. This method is fast and stable but, being purely
  based on mean and volatility, ignores any skew or fat tails actually present
  in the return history — it will typically understate risk for a fund whose
  losses are more extreme or more asymmetric than a normal distribution would
  predict.
- **Cornish-Fisher** — starts from the same normal-distribution approach as
  Gaussian, but corrects the result for the skewness (asymmetry) and excess
  kurtosis (fat tails) actually measured in the window's returns. When the
  portfolio's return history is close to symmetric and normally shaped, this
  method converges to the Gaussian figure; when it is skewed or has fatter
  tails than a normal distribution, Cornish-Fisher typically produces a
  larger (more conservative) VaR/ES than Gaussian.

The three methods will generally disagree the most when recent returns have
been unusually skewed or have included a few large outlier days — Historical
picks that up directly, Cornish-Fisher partially adjusts for it, and Gaussian
will tend to be the most optimistic (smallest) of the three in that case.

The fourth card, **Limit utilization**, expresses the Historical VaR as a
percentage of the UCITS limit (Historical VaR ÷ limit). It is shown in red
whenever utilization exceeds 100% (the fund is over its absolute VaR limit)
and in green otherwise.

## Rolling VaR chart

Below the cards, a line chart plots the Historical-method VaR, at the
currently selected confidence and horizon, recomputed each day using a
trailing window of the same length as the Window control (shrunk to however
much history is actually available, if less). A dashed red horizontal line
marks the UCITS limit. Where the VaR line crosses above that dashed line, the
fund was over its regulatory VaR ceiling on that date.

## Limit breaches table

Underneath the chart, a table lists every date on which the rolling VaR
(described above) exceeded the UCITS limit, with the VaR value on that date.
If there were none, the tab simply states "No breaches over the computed
history."

## Back-testing

The back-testing section validates the VaR estimate against what actually
happened, independently of the controls at the top of the page. It always
uses a **1-day horizon at 99% confidence**, with the fund's configured
trailing window (the same "Window" default shown above, not whatever value is
currently selected in the control). For each day in the available history, it
computes what the 1-day 99% VaR would have been from the preceding window of
returns, under each of the three methods, and checks whether the actual
realized return on that day was worse than (i.e. lost more than) that
predicted VaR. A day where the loss exceeds the predicted VaR is called an
**exception**.

For each method, a card reports:

- The **exception count** out of the number of days tested (`exceptions / n`).
- A **traffic-light zone**, following the Basel framework for backtesting VaR
  models, based on the exception count over the most recent 250 tested days
  (or fewer, if less history is available):
  - **Green** — 4 or fewer exceptions. The model's exception rate is
    consistent with a well-calibrated 99% VaR.
  - **Yellow** — 5 to 9 exceptions. More exceptions than expected; worth a
    closer look, but not conclusive evidence the model is wrong.
  - **Red** — 10 or more exceptions. Substantially more exceptions than a
    99% VaR should produce; the model is under-predicting losses.
- The **Kupiec p-value** — the result of the Kupiec proportion-of-failures
  test, a statistical test of whether the observed exception rate is
  consistent with the claimed 1% exception rate (i.e. 99% confidence). A
  low p-value means the observed exceptions are unlikely to have occurred by
  chance if the model's stated confidence were correct — in practice, read a
  p-value **below 5%** as the model failing the check, and the card marks
  this explicitly with "model rejected" next to the p-value. A higher
  p-value means the test found no statistical evidence against the model at
  its stated confidence, even if the zone is yellow. If the test could not be
  computed, the card shows "n/a".
- **"partial: N/250"** appears next to the p-value whenever fewer than 250
  days have been tested yet — the zone and p-value are still computed, but
  over a shorter history than the usual one-year reference period, so they
  are less statistically conclusive until more history accumulates.

A yellow or red zone, or a rejected Kupiec test, does not necessarily mean the
VaR model is broken — a short run of unusually volatile days can trigger it —
but it is a signal to check whether that method's assumptions (e.g. the
Gaussian shape, or the trailing window length) still fit the fund's recent
return behaviour, and to weigh the Historical or Cornish-Fisher figures more
heavily if Gaussian is the one flagged.

Below the three cards, a chart plots the fund's actual daily returns against
each method's negative VaR threshold for that day (drawn as three lines,
"−VaR hist", "−VaR gauss", "−VaR CF"). Any day where the return line dips
below one of those threshold lines is an exception for that method, and is
additionally marked with a red dot on the chart.

### When there isn't enough history

The back-test needs more daily returns than its trailing window is long —
with the default 252-day window, at least 253 daily returns — before it can
test even a single day. Below that, the back-testing section shows
"Insufficient history for back-testing" instead of the cards and chart, and
states how many daily returns are needed.

Separately, the method cards and rolling-VaR chart at the top of the page have
their own, lower bar: at least 30 daily returns are required to compute any
VaR/ES figures at all. Below that, the cards show "–" for every method and a
warning badge reports how many observations are actually available. Between
30 returns and a full window's worth, the cards still compute normally but
another warning badge notes that the window was shrunk to the available
history rather than the full configured length.
