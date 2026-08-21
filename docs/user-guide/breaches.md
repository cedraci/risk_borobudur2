# Breaches

The Breaches tab is the fund's persistent record of every limit check the tool runs, kept over
time rather than on a single snapshot. Where the Limits and Derivatives pages show you today's
verdict, this page shows the history behind it: every check the tool has ever run for this
portfolio, and every stretch of time a check spent in breach, tracked from the moment it opened to
the moment someone signs off on it.

It covers every check the tool computes a limit for — the UCITS concentration checks, the liquidity
redemption scenarios, the VaR limit and the EMIR clearing-threshold classes (fifteen checks as this
is written, and whatever the tool checks by the time you read it) — the same figures shown on the
Limits, VaR/ES and Derivatives pages, but recorded here as a dated,
unchangeable history rather than a point-in-time read.

## Runs

A **run** is one pass of every check against one NAV date, recorded in full — every check's
status, not just the ones that breached. A run happens automatically every time you import a new
position snapshot for the portfolio, using that snapshot's own date. Re-importing a file the tool
has already seen records nothing new, and recording a run is deliberately not allowed to fail an
import: if the register cannot be written the upload still succeeds and the failure is logged
server-side, so an import that produces no new run here is a reason to check the server log rather
than to re-import. You can also trigger one by
hand with the **Re-run checks now** button, for example after tightening a limit and wanting
today's verdict without waiting for the next import.

**A run is immutable.** Nothing in the tool ever edits or deletes a recorded run — if you change a
limit tomorrow, yesterday's run still shows yesterday's verdict against yesterday's limit.

**Re-running re-checks the latest snapshot — it is not a way to rebuild history.** The button (and
the endpoint behind it) always re-checks the most recently imported snapshot date; there is no way
to ask it to recompute an older date. This is deliberate: a run for an earlier date would compute
findings from that day's holdings and then close every episode that is not present in them —
stamping a breach as "cleared" that, on the real, current data, never cleared at all. If you need
to see what an old snapshot's checks looked like, read it from the run history below rather than
re-running it.

## Episodes: why six weeks of breach is one row, not forty-two

A single check breaching on a single date is a row in a run. But the same issuer being over its
10% limit for six straight weekly imports is not six separate problems — it is one problem that
has not gone away, and the register tracks it that way. The first time a check breaches for a
given subject (an issuer, a bank, "Historical VaR", an EMIR asset class, and so on) with no episode
already open for it, the register opens an **episode**. Every later run that finds the same check
still breaching for the same subject updates that one episode rather than opening another — raising
its recorded **peak** if the new reading is worse than any seen before, but not creating a new row.
Only when a run no longer finds that subject breaching does the episode close.

An episode remembers:

- **when it opened** — the NAV date of the run that first saw the breach, and the value observed
  then;
- **its peak** — the worst value seen across every run while it stayed open. Some checks have no
  single number to report — the liquidity redemption scenarios are a pass/fail on a whole
  redemption profile, not one ratio — and those episodes show a dash rather than an invented
  figure;
- **when it cleared**, if it has — the NAV date of the first run that no longer found it breaching;
- its **state** and **classification** (below);
- who acknowledged it and when, with their note; and who resolved it and when, with theirs.

A breach that clears and then recurs later is a genuinely new episode, not a reopening of the old
one — a fund going back over a limit next month is a fresh thing to explain, even if last month's
breach of the same limit was never formally resolved.

## Cleared on the data, and resolved by a person — not the same thing

A run clearing an episode (the check no longer breaches) and a person **resolving** it are two
different things, and the page is careful to keep them visually distinct:

- **Cleared on the data** means the numbers moved — the position was trimmed, the market moved, NAV
  changed — and the most recent run no longer finds a breach. This happens automatically, with no
  one involved.
- **Resolved** means a person looked at the episode, classified it, and signed off that it is
  closed.

An episode that has cleared on the data but that nobody has resolved shows a line reading **"cleared
on the data since \<date\> — awaiting sign-off"**, distinct from the green "Resolved" state. This
exists so a breach can never quietly disappear from view just because the market happened to move
back in the fund's favour before anyone looked at it — someone still has to confirm it.

## Classification: Proposed is not a decision

Every open episode carries a **classification**, starting as **Unclassified**, and shown as its own
chip separate from the episode's state chip. Unclassified is not a status you can leave in place
indefinitely if you want the episode acknowledged — it simply means nobody has recorded a judgement
yet.

Alongside Unclassified, the card shows what the tool itself thinks happened, from comparing the
subject's holdings between the previous snapshot and this one:

- **Proposed: Active** — the fund bought more of the subject since the last snapshot, so the breach
  is (at least in part) a deliberate trading decision.
- **Proposed: Passive** — nothing was bought; the breach is attributed to market movement (prices,
  NAV, or other positions changing weight) rather than a trade.
- **No proposal — \<reason\>** — the tool declines to guess. This happens in three cases: the
  episode opened on the portfolio's very first imported snapshot (there is no earlier one to compare
  against); the subject has no instrument holdings to compare at all (it is not derived from
  positions); or one of the subject's instruments has no reported quantity at one of the two
  snapshots, so the tool cannot rule out a purchase having happened. Each of these reads a specific
  reason next to it, so a person can tell at a glance why the engine could not say.

Every one of these is a **proposal**, never a decision. Where the tool has an opinion the wording
always reads "Proposed:"; where it does not, it says so outright. Neither is a classification. The classification chip stays
**Unclassified** until a person acts. This cuts both ways: a real proposal must never be presented
as though it had already been decided, and a declined proposal must never be quietly defaulted to
"Passive" just because that reads as the safer of the two — an invented guess would be worse than
admitting the data does not support one.

## Acknowledging and resolving

An **open** episode can be acknowledged. Acknowledging is the point where a proposal becomes a
decision: it requires choosing a real classification — **Active** or **Passive** — and writing a
note; the classification defaults to neither, and an empty note is refused. You can optionally set
a deadline date. Once acknowledged, the episode's state chip changes and the card records who
acknowledged it, when, and their note.

An **acknowledged** episode can be resolved, which also requires a note. The page only offers the
resolve control once an episode has been acknowledged, and the server refuses an out-of-order
resolve as well — acknowledging is not a formality you can skip on the way to closing something
out.

Both the acknowledging and resolving user's name is recorded at the moment they act, not looked up
later, and never looked up again — so the record of who acted still reads correctly however that
account changes afterwards: renamed, disabled, or stripped of every grant. That permanence is part
of what makes the register usable as evidence.

## Breach episodes and Run history on the page

The page has two sections.

**Breach episodes** lists the whole register for the portfolio — every episode ever opened, not
only the open ones — sorted with open episodes first, then everything else by the date it opened.
A resolved episode still appears here; only its state chip and its position further down the list
say it is no longer live.

**Run history** is a table of every recorded run, one column per run (newest first, each headed
with that run's NAV date) and one row per check, with the check's status (OK/WATCH/BREACH) in each cell — a grey **N/A** where a
check could not be evaluated for that run because an input was genuinely missing (for example, no
shareholder register loaded, so the two "Top 5 holders" liquidity scenarios could not run). Hover
a column with an "incomplete" flag to see which inputs were missing and why. **Re-run checks now**
sits above this table and triggers a fresh run, described above. The on-page table shows the most
recent runs only (52 at the time of writing); the evidence export below has no such limit and
includes the full history however far back it goes.

## The evidence export

**Export evidence workbook** downloads an `.xlsx` file named
`Breach register - {portfolio name} - {date}.xlsx`. It covers the same two sections as the page —
the whole register and the whole run history, with no cap on how far back it goes — in two
worksheets.

Note that `{date}` is the day you downloaded the file, in UTC, not the snapshot date. The EMIR
evidence export is named for its anchor date instead, so the two files sort differently; file these
by the date inside them rather than the one in the name.

The worksheets are:

- **Register** — one row per episode: the check, the subject, when it opened, its peak value,
  whether (and when) it cleared, its state and classification, who acknowledged it and when with
  their note, who resolved it and when with their note. It records what was *decided*, not what the
  tool guessed: the proposed classification and its reason, the value at which the episode opened,
  and any deadline are on the page but not in this sheet. Checks appear under their internal keys
  (`issuer_10`, `liq_fixed`, `emir_credit`, …) rather than the page's labels.
- **Run history** — one row per recorded run (its NAV date, when it ran, and what triggered it) and
  one column per check the tool has ever run for this portfolio, with the status in each cell —
  blank where a check was absent from that particular run.

As above, the acknowledged-by and resolved-by names in the export are the names captured at the
time of the act rather than looked up when the file is built, so the file still shows who acted
whatever has happened to that account since — which is exactly what makes it usable as evidence for
a regulator or an auditor.

## Access rights

Viewing the Breaches page — the episode list and the run history — needs `settings/view` on the
portfolio. Re-running the checks by hand, acknowledging, and resolving all need `settings/configure`.
Downloading the evidence workbook needs `settings/export`. That grant already carries the right to
view the domain — like every export, import and configure grant in the tool — so `settings/export`
alone is enough to open the page and download the file; it does not have to be paired with a
separate view grant. See [Access rights](access-rights.md) for how these grants and domains work
across the tool.
