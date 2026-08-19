# Data

The Data tab is where every number in the rest of the tool comes from. It is the page you use to
bring in the weekly NAV workbook or the depositary's own daily files, confirm what the tool has
guessed about the futures you hold, feed it classifications and FX rates from a Bloomberg
Terminal, edit reference data by hand, manage the list of portfolios, and maintain the shareholder
register. Nothing on this page is computed risk analytics — everything here is either raw data
coming in, or the small amount of reference data and configuration the rest of the tool depends on.

Most panels on this page apply to the portfolio currently selected in the navigation bar; a few —
reference data, futures contract specs, and the Bloomberg panel — are shared across every
portfolio and say so under their heading. See "What is portfolio-scoped and what is shared" near
the end of this chapter for the full picture, and [Access rights](access-rights.md) for how
permissions are structured — imports, reference-data edits, and views of the underlying data are
governed by separate, independently-grantable rights, so what a given person can do on this page
can vary panel by panel.

## The weekly workflow

This is the end-to-end routine for a normal week, combining a NAV Recap upload with the CTD and
Bloomberg steps that ride alongside it. If the portfolio is instead fed daily from the depositary,
see "The CACEIS CSV feed" below for how that path differs.

1. **Pick the portfolio** in the navigation bar before uploading anything. Files you drop route to
   whichever portfolio you have currently selected — except CACEIS files, which identify their own
   fund from the filename and route through the CACEIS code mapping set on the Portfolios panel,
   regardless of which portfolio happens to be selected. If the portfolio you have selected is
   archived, the whole upload is refused up front — even for CACEIS files that would otherwise
   route somewhere else — so make sure an active portfolio is selected before dropping anything.
2. **Upload the NAV Recap workbook** on the Import panel. Any new futures contract found in the
   file is seeded with a point value derived from the workbook and flagged unconfirmed; open the
   Futures contracts panel and confirm each one, setting its category, curve and price convention.
   US Treasury futures are quoted in 32nds on the portfolio sheet — set their price convention to
   **th32** so the point value is decoded correctly. Re-uploading a workbook already on record does
   not re-import it, but it does still seed any contract specs that are still missing — so
   re-uploading the same file is the fix for a futures contract table that is unexpectedly empty.
3. **Upload the CTD companion file** for the same NAV date, on the Futures contracts panel.
   Re-uploading for a date already on record replaces that date's rows, so a corrected pull simply
   overwrites — there is no separate delete step.
4. **Export the Bloomberg request workbook** on the Bloomberg classification panel. It lists every
   equity, fund or bond position still missing a country or sector — bonds only need a country,
   since Bloomberg publishes no GICS sector for corporate or government securities — plus every
   non-EUR currency currently held anywhere in the fleet.
5. **Fill it in on a machine with a Bloomberg Terminal.** The workbook's formulas only resolve
   inside Excel on a machine with a logged-in Terminal add-in; open it there, wait for the formulas
   to calculate, and save it. Nothing in the tool itself queries Bloomberg.
6. **Upload the resolved workbook back** on the same panel. This stores country, region, sector,
   industry and ticker for every instrument that resolved, stores the FX rates, and cross-checks
   each rate against the NAV Recap's own currency movement at every date it applies to — a mismatch
   beyond 1% usually means Bloomberg returned the inverted quote and is reported rather than stored
   silently. Cells that never resolved (blank or `#N/A`) are skipped and listed, not guessed at.

## Portfolios

At the top of the Data page, the Portfolios panel lists every portfolio — active and archived — and
lets you create, rename, archive/restore one, and set the depositary code a CACEIS-fed portfolio
routes on.

The table has these columns:

| Column | Meaning |
| --- | --- |
| Name | The portfolio's display name, shown everywhere else in the tool (navigation bar, reports, exports). Click the pencil icon next to it to rename. |
| Kind | Either **ucits** or **mandate**, set once at creation and not editable afterwards. |
| CACEIS code | The depositary fund code this portfolio is mapped to, editable inline (see "Mapping a CACEIS code" below). Blank if the portfolio has none. |
| Latest NAV | The date of the most recent NAV on record for the portfolio, or a dash if none has ever been imported. |
| Status | **active** or **archived**. |
| (actions) | Rename (while editing: Save/Cancel), and Archive or Restore. |

### Renaming a portfolio

1. Click the pencil icon next to the name.
2. Edit the text field that appears in place of the name.
3. Click **Save** to apply the new name, or **Cancel** to discard the edit. The name cannot be
   blank, and it must not collide with another portfolio's name — either problem is reported in
   place rather than applied.

Renaming only changes the display name; it never touches the portfolio's archived status.

### Archiving and restoring a portfolio

Click **Archive** on an active portfolio, or **Restore** on an archived one — this is a single
click with no confirmation dialog, and it only flips the status flag, leaving the name untouched.

An archived portfolio:

- **Stays fully readable.** Every page still shows its historical data, so nothing is lost by
  archiving.
- **Refuses new data.** Any import, CTD upload, or reference edit scoped to a mutating action on
  that portfolio (including the shareholder register and portfolio settings) is rejected with a
  message naming the portfolio as archived.
- **Drops out of the navigation-bar selector**, so you can no longer pick it as the active
  portfolio from day-to-day navigation, and it stops appearing in the futures contracts and
  Bloomberg panels' "all active portfolios" sweeps — for example the Bloomberg request workbook and
  its FX cross-check only ever cover active portfolios' latest snapshots.

### Creating a portfolio

Under **Create a portfolio**, fill in a name, choose a kind (**ucits** or **mandate**) from the
dropdown, and click **Create**. The name must be non-blank and unique; a duplicate name is
rejected with a message rather than silently overwriting the existing portfolio. A new portfolio
starts empty, with no CACEIS code mapped and no data of any kind — the next step for a
CACEIS-fed one is normally to map its code (below), then start dropping files.

### Mapping a CACEIS code

The **CACEIS code** cell for each portfolio is a small text field with its own **Save** button,
enabled only once you have typed something different from the currently saved value. Type the
depositary's numeric fund code and click Save. This is the mapping the CACEIS CSV feed uses to
route a self-identifying file to the right portfolio (see below) — a portfolio with no code mapped
here cannot receive CACEIS files at all; any file coded to it comes back rejected, pointing back to
this panel. Clearing the field and saving removes the mapping.

### Access rights on this panel

Renaming a portfolio, archiving/restoring one, and editing its CACEIS code all require configure
rights on reference data — either for that specific portfolio or for all portfolios. Creating a
portfolio is stricter: since the new portfolio does not exist yet, only an all-portfolios
reference-data configure grant allows it — a grant scoped to specific portfolios, however many,
does not. Everyone who is signed in can see the list itself, but it only shows the portfolios
their own permissions actually cover; a portfolio entirely outside your scope does not appear at
all. See [Access rights](access-rights.md).

## Importing files

Below the Portfolios panel, the **Import** card is a single drop zone (also usable as an ordinary
file picker) that accepts any mix of `.xlsx` and `.csv` files in one go. You can drop a NAV Recap
workbook and one or more CACEIS CSV files together — each file is detected, routed and reported
independently, so mixing file types in a single drop is normal and safe.

The panel's heading names the portfolio currently selected — that is where any file that cannot
identify its own fund (i.e. the NAV Recap workbook) will land. While an import is in progress the
panel reads "Importing…" and the file picker is disabled until it finishes.

### The NAV Recap workbook

An `.xlsx` NAV Recap workbook always lands in the portfolio you have selected in the navigation
bar — it carries no fund identifier of its own. Uploading one:

- Records that date's NAV, AUM, and shares outstanding.
- Replaces that date's full position snapshot.
- If it is the most recent file carrying a dividend/operations journal seen so far for this
  portfolio, replaces the whole dividend and trade journal with what this file contains — see "How
  re-uploads and older files behave" below.
- Seeds a contract spec for every new futures root it holds (see "Futures contracts" below), and
  cross-checks the implied point value against the spec already on file for a root it already
  knows.
- Parses coupon rate, maturity and frequency out of a bond position's own name wherever those
  fields are still blank in reference data (see "Reference data" below).

**Re-uploading a workbook already on record** (byte-for-byte identical to a file already imported
for this portfolio) does not re-import anything — no NAV, position, dividend or trade data changes
— but it still runs the futures-contract seeding step. This makes re-uploading the safe repair for
a futures contract table that is unexpectedly empty: nothing else is disturbed.

### How re-uploads and older files behave

Only the single most recent file (by NAV date) that carries a dividend/operations journal is
allowed to replace the stored journal. If you upload an older workbook after a newer one has
already been imported, its NAV and position data are still recorded normally, but its
dividend/operations rows are left out — the result line for that file says
**"(older file: dividends/operations left untouched)"** so this is never silent. CACEIS files never
carry this journal at all, so this note never applies to them.

### The CACEIS CSV feed

A portfolio administered by CACEIS Bank Luxembourg can be fed directly from the depositary's own
daily exports, instead of — or alongside — the weekly NAV Recap workbook. Every CACEIS file
identifies its own fund from a code embedded in its filename and routes through the CACEIS code
mapping set on the Portfolios panel, regardless of which portfolio you have selected. A file coded
to a fund with no mapping is rejected with a message pointing you back to that panel.

The filenames the tool recognizes, all following the pattern `<PREFIX>_<fund code>_<yyyymmdd>_<timestamp>.csv`:

| Filename prefix | What it carries | Outcome |
| --- | --- | --- |
| `HISINVLUX` | The day's full position snapshot. | Imported. |
| `HISTOVLLUX` | NAV, total net assets, and shares outstanding for the day. | Imported. |
| `JOURSRLUX` | Daily subscriptions and redemptions per share class. | Imported — feeds the observed-outflow comparison shown against the configured redemption stress (see the Limits chapter's "Observed flow history"). |
| `INVJCPLUX` | Bond coupon-payment frequency detail. | Imported — the only field this file contributes is coupon frequency, since the maturity and coupon-rate columns it also carries have no confirmed layout and are deliberately left unused rather than risk silently overwriting a value already trusted from `HISINVLUX`. |
| `INVXDVLUX` | Positions again, in a different layout. | **Declined as redundant** — `HISINVLUX` already carries the positions. |
| `JOUROPLUX` | The trade journal (buys/sells). | **Declined, pending a sample file** to build its parser against. Until it is supported, a CACEIS-fed portfolio has no trade journal at all — see the note under "What CACEIS gives you and what it doesn't", below. |
| `REGLMTLUX`, `RAPDECLUX` | Settlement ledger / detached dividend detail. | **Declined** — recognized but not yet consumed; everything they would add is already present under a different name in the snapshot (pending trades, accrued fees, dividend-receivable positions), and importing them without a confirmed de-duplication rule risks double-counting. |

A file whose name does not match any of these prefixes, or whose content does not look like the
layout its prefix implies (wrong column count, mismatched date or fund code between the filename
and the rows), is rejected outright — nothing from it is stored.

**Drop one or more CACEIS files together** and each comes back with its own line in the results
table: the file kind detected, the portfolio it routed to, and either the row counts imported or
the rejection reason — one file's rejection never stops the others in the same drop from being
processed.

### What CACEIS gives you and what it doesn't

- **AUM cross-check.** Whenever a position snapshot and a NAV/AUM figure now both exist for the
  same date (whether from the same import or built up across several), the tool sums the position
  values and compares the total against the file's own AUM. A drift beyond 0.1% is reported as a
  warning — it does not block the import, but it is worth investigating (a truncated position file
  or a stale NAV figure are the usual causes). This check runs for any import that touches a date
  with both pieces present, not only CACEIS ones.
- **Dividends are derived, not read.** A CACEIS-fed portfolio has no explicit dividend journal
  (that would come from `JOUROPLUX` or the NAV Recap, see above), so dividends are instead inferred
  from the growth of dividend-receivable positions between two consecutive snapshots, and stored
  flagged as derived. If an explicit dividend journal for the same date ever arrives (typically from
  a NAV Recap import covering the same portfolio), it always wins and suppresses the derived entry
  for that date.
- **No trade-level P&L attribution.** With no trade journal, the P&L page cannot split a
  CACEIS-fed portfolio's period result into realized and unrealized pieces — see the P&L chapter's
  note on this.
- **Reference data pre-fill.** A CACEIS position that carries a risk country or a Bloomberg ticker
  fills those fields — plus the geographic region implied by the country — into the shared
  reference data, but **only where the field is still blank**. A value you or a Bloomberg upload
  already confirmed is never overwritten by this. A bond classified this way needs no sector to
  count as fully classified (Bloomberg publishes none for corporate/government bonds), so it drops
  off the Bloomberg request workbook as soon as its country is known.
- **Bond statics are restated on every import.** Market place, maturity, coupon rate, nominal
  amount and (from `INVJCPLUX`) coupon frequency are treated as authoritative from the depositary
  and **overwrite** whatever is currently stored, whenever the incoming file has a value for them.
  This is the opposite rule from the pre-fill above: because the depositary restates these fields
  on every import, hand-editing coupon or maturity for a CACEIS-sourced bond in the Reference data
  panel is effectively futile — the next import simply restores the depositary's own value.

### Import results and messages

Every file you drop — NAV Recap or CACEIS — produces one row in the results table under the drop
zone, with these columns:

| Column | Meaning |
| --- | --- |
| File | The filename as uploaded. |
| Kind | The file type detected (NAV Recap, or the specific CACEIS format), or a dash if detection failed. |
| Portfolio | The portfolio the file was routed to and imported into, or a dash if it never got that far. |
| Result | Success text and any warnings, or the rejection/error message. |

**Success outcomes:**

- **"Already imported (identical file)."** — the exact same file (byte for byte) has already been
  recorded for this portfolio; nothing changed. Futures contract seeding still runs even in this
  case.
- **"Imported: *N* NAV rows, *N* positions, *N* dividends, *N* operations."** — the normal outcome.
  For a NAV Recap file whose dividend/operations journal was superseded by a newer file already on
  record, this is followed by "(older file: dividends/operations left untouched)" (see above);
  CACEIS files never carry this suffix, since they never carry that journal.
- Below the outcome, any number of small warning badges may appear — informational notes that never
  block the import. Examples include the AUM cross-check drift described above, a new futures
  contract seeded and needing confirmation, a point-value mismatch against an already-known
  contract's spec, the count of dividend events derived from receivable growth, and a data-quality
  note when a cash-holding position's trade history in the same file does not fully explain its
  reported average cost (an incomplete trade history, an oversold position, or a cost-basis
  mismatch beyond a small tolerance).

**Rejections and errors:**

- An unrecognized filename or content, a CACEIS file whose fund code has no portfolio mapping, or a
  file rejected outright by kind (see the table above) shows its reason directly in the Result
  column.
- A file that is the right kind but fails validation row-by-row shows an error summary plus a small
  table underneath listing up to the first 10 problem rows, each with its sheet, row number, and
  the specific problem.
- A file you are not permitted to import into its target portfolio (whether the portfolio you
  selected, for a NAV Recap, or the portfolio a CACEIS code resolved to) is refused for that file
  alone with a message naming the file and the missing permission — other files in the same drop
  are unaffected. See [Access rights](access-rights.md).

## Import history

Below the Import panel, a plain table lists every file ever successfully imported for the currently
selected portfolio, most recent first:

| Column | Meaning |
| --- | --- |
| File | The filename as it was uploaded. |
| NAV date | The date the file's own data is keyed to. |
| Imported at | The exact date and time the import happened. |
| Rows | A comma-separated summary of what was stored (for example "nav_rows: 1, positions: 84"). |

This is a read-only log — there is nothing to edit or delete here, and it is unaffected by a
portfolio being archived (history stays inspectable). Viewing it requires the same reference-data
viewing right as the reference data editor below.

## Weekly CTD companion file

Bond futures need a small extra file every week to compute their DV01: the cheapest-to-deliver
(CTD) analytics, since these are not present in either the NAV Recap or the CACEIS feed. This
upload lives on the **Futures contracts** panel, in the **Weekly CTD analytics** section below the
contract table, and is scoped to the currently selected portfolio.

The file is one row per bond future, either `.csv` or `.xlsx` (for a spreadsheet, the tool reads a
worksheet literally named `CTD`, falling back to the first sheet if there isn't one), with these
columns, in any order:

| Column | Meaning |
| --- | --- |
| `nav_date` | `YYYY-MM-DD`. Must be identical on every row, and must match a NAV date already on record. |
| `ticker` | The Bloomberg-style futures ticker (for example `TYU6 Comdty`). Must match a future actually held in that date's snapshot. |
| `ctd_isin` | ISIN of the cheapest-to-deliver bond. |
| `ctd_mod_duration` | Modified duration of the CTD bond. |
| `ctd_clean_price` | Clean price of the CTD bond. |
| `ctd_accrued` | Accrued interest of the CTD bond (zero is a valid value). |
| `conversion_factor` | The contract's conversion factor for that bond. |

**To upload:** select the file with the file picker under "Weekly CTD analytics." There is no drag
zone here, just a single-file picker.

- **All-or-nothing validation.** Every row must reference a NAV date already recorded and a ticker
  actually held as a future in that date's snapshot; if any row fails, the whole file is rejected
  with a table naming the offending row, column and problem (up to the first 20), and nothing from
  the file is stored. Fix the file and re-upload.
- **Re-uploading for a date already on record replaces that date's rows entirely.** A corrected
  pull is simply a re-upload — there is no separate delete step, and no partial merge.
- On success, a confirmation states the row count stored and the NAV date, noting when it replaced
  an existing set.
- If the portfolio has no snapshot at all for the date in the file, or you cannot view its
  positions, the upload is rejected with a message explaining which of the two applies (no snapshot
  yet, versus a permission gap) rather than one generic error covering both.

Below the upload control, a table lists the CTD rows currently stored for the most recent date with
any positions and permission to view them: **Ticker**, **CTD ISIN**, **Mod. duration**, **Clean**,
**Accrued**, and **Conv. factor**.

**CTD analytics are never carried forward from a previous week.** A NAV date with no matching
upload shows that future's notional normally elsewhere in the tool but reports its DV01 as missing
for that date — see the Limits chapter's "Bond futures" section.

## Futures contracts

This panel, also on the Data page, lists every futures contract root the tool knows about — shared
across every portfolio, since a given contract (say, the 10-year German Bund future) behaves
identically no matter which fund holds it.

### How specs get here

A contract spec is normally **seeded automatically** the first time a NAV Recap workbook holding
that contract root is imported: the point value is derived from the workbook's own price and
valuation figures, the category is guessed only where the ticker suffix says so unambiguously
(an "Index" suffix implies equity, a "Curncy" suffix implies FX; anything else, including
"Comdty" — which covers both bond and commodity futures — is left as "Other"), and the row is
marked **unconfirmed**. A banner at the top of the panel counts how many specs still need
confirming whenever there are any.

If the table is unexpectedly empty for a fund that holds futures, re-upload its NAV Recap workbook
— re-uploading a file already on record still runs the seeding step even though nothing else is
re-imported (see "The NAV Recap workbook" above).

### The contract table

| Column | Meaning |
| --- | --- |
| Root | The futures contract root (read-only). |
| Label | A free-text display name. |
| Category | One of Equity, Interest rate, Foreign exchange, Credit, Commodity, Other. |
| Point value | The EUR (or contract-currency) value of one full point of price movement. |
| Ccy | The contract's currency. |
| Curve | A free-text label for the interest-rate curve it belongs to (optional, mainly used for interest-rate contracts). |
| Price convention | **decimal** or **th32 (32nds)** — see below. |
| OTC | A checkbox: tick it if this contract trades bilaterally or on a venue not recognized as equivalent, so it counts as OTC for EMIR clearing-threshold monitoring. Left unticked, a contract is assumed exchange-traded and non-OTC. |
| Status | **confirmed** or **unconfirmed**. |
| (actions) | Save (any field), and Confirm / Unconfirm. |

Editing any field enables its row's **Save** button; saving keeps the row's current confirmed
status untouched unless you also use Confirm/Unconfirm.

- **Confirm** marks the row confirmed without changing any other field — use it once you have
  checked the guessed category, curve and price convention (and point value) are correct.
- **Unconfirm** asks for a one-click confirmation first, since it pulls the contract back into
  being treated as unverified — including, if its category is not interest rate, back into the
  rates DV01 section on the Limits page until someone confirms it again.

**Price convention and th32:** US Treasury futures are quoted in 32nds of a point on the NAV Recap
sheet, not decimal. Leaving the convention on the default "decimal" for one of these
misinterprets its price and average cost, which throws off the point-value cross-check described
next. Set it to **th32** for every US Treasury root.

### The point-value cross-check

Every time a NAV Recap workbook is imported for a contract root already known, the tool re-derives
the point value implied by that week's price and valuation figures (decoded using the stored price
convention) and compares it against the stored value. Within half a percent, nothing is reported.
Beyond that, a warning names the root and states the stored value against the implied one — and if
the *other* price convention would have reconciled the two, the warning says so explicitly ("point
value implies convention th32, stored decimal"), which is usually the fastest way to spot a
Treasury future still left on the wrong convention.

### Adding a contract root manually

Click **"Add a contract root manually"** to open a one-row form with the same columns as the main
table, plus the contract root itself (free text, e.g. "RX"). This is explicitly an escape hatch,
not the normal path — a hand-typed spec is never cross-checked against a workbook's own figures the
way a seeded one is. It exists for the case the importer genuinely could not derive a spec: a
futures position with no ticker, an unparseable root, or a row too incomplete to imply a point
value. A manually added root:

- Must not already exist in the table (you are told to edit the existing row instead if it does).
- Lands unconfirmed, exactly like a seeded row.
- Uses "Other" and "decimal" as its category and price-convention defaults, which you should review
  before confirming it.

Click **Add** to save it, or **Cancel** to discard the form.

### Access rights

Viewing the contract table and the CTD records requires reference-data / market-data viewing
rights; editing a contract spec (Save, Confirm, Unconfirm, or adding one manually) requires
reference-data configure rights, held globally rather than per portfolio, since contract specs are
shared across the whole fleet. Uploading the CTD companion file requires market-data import rights
on that specific portfolio. See [Access rights](access-rights.md).

## Bloomberg classification

This panel, shared across every portfolio, is where the round-trip with a Bloomberg Terminal
happens: exporting a request workbook of formulas, and uploading it back once those formulas have
resolved in Excel.

### Exporting the classification request

Click **"Export Bloomberg request"** to download an `.xlsx` workbook. It is built by walking every
active (non-archived) portfolio at its own most recent snapshot, and contains:

- **One row per instrument** (equity, fund, or bond) still missing a country, or — for equities and
  funds only — still missing a GICS sector. A bond only needs a country to be considered done,
  since Bloomberg publishes no GICS classification for corporate or government securities; requiring
  a sector for a bond would keep re-listing it forever. Instruments are de-duplicated across the
  fleet, so a bond held by three portfolios appears once.
- Formulas that key off `{ISIN} {market sector}` — the market sector ("Equity" for equities and
  funds, "Corp" for bonds) is written as a plain, editable cell next to each row, so if a particular
  bond needs "Govt" instead of "Corp" to resolve, you fix that one cell in Excel rather than
  touching any formula.
- **One FX request per non-EUR currency** currently held anywhere across the fleet, spanning the
  full date range covered by the portfolios' own imported history.
- A README sheet inside the workbook itself repeating the round-trip instructions.

### Filling it in and uploading it back

1. Open the exported file in Excel **on a machine with a logged-in Bloomberg Terminal add-in** —
   the formulas only resolve there; nothing in the tool queries Bloomberg directly.
2. Wait for every formula to calculate.
3. Save the file, keeping the `.xlsx` format.
4. Upload it back using the file picker on this panel.

On upload, the tool:

- Stores country, region (derived from the country), GICS sector, GICS industry and the resolved
  ticker for every instrument row that came back with a value.
- Stores every FX rate that resolved.
- **Cross-checks each FX rate** against the NAV Recap's own currency-movement figure, at every
  snapshot date across every active, viewable portfolio where that currency and date both apply. A
  rate whose drift against the workbook's own figure exceeds 1% is reported — this is usually the
  sign that Bloomberg's quote came back inverted rather than in the expected direction, so it is
  flagged rather than silently stored as-is.
- Lists every cell that never resolved (blank, `#N/A`, or a similar Bloomberg error value) rather
  than guessing at it — these stay unclassified until a corrected pull is uploaded.

### Reading the result

After an upload, the panel shows:

- **"*N* instrument(s) classified."** — or, if you are not permitted to configure reference data,
  an unavailable notice explaining that classification was never attempted (never shown as a real
  zero, which would look like a check that found nothing to do).
- **"*N* FX rate(s) stored, *N* ADV volume(s) stored."**
- If there is nothing to report, "No skipped cells and no FX cross-check drift."
- **Skipped cells**, if any: a warning count plus a table of up to 20 (sheet, row, reason).
- **FX cross-check failures**, if any: a table of currency, date, the workbook's own rate, the
  Bloomberg rate, and the drift percentage.
- If the FX cross-check could not be run for one or more portfolios because you cannot view their
  positions, a note names which portfolios were skipped — the absence of drift shown above does not
  cover those.

### The ADV request

Below the classification export, a second export — **"Export ADV request"** — targets 30-day
average trading volumes instead, used for liquidity capacity. It shows how many instruments across
the fleet are currently due for a refresh, out of how many are held in total ("*N* of *N* due").
Tick **"full rebuild"** to export every eligible instrument instead of only the ones due; leave it
unticked for the normal weekly refresh of stale figures. This is a separate workbook from the
classification request (its own `ADV` sheet, uploaded through the same file picker above), since
volumes decay daily while classifications, once resolved, mostly stay resolved.

A volume figure is treated as stale once it is older than the portfolio's configured maximum ADV
age (see "Settings" below); a stale or never-fetched figure is what makes an instrument "due."

### Access rights

Exporting either request requires export rights on positions, held globally across the fleet.
Uploading the response requires market-data import rights, also global; the classification columns
specifically (country, sector, industry) additionally require reference-data configure rights — a
principal with only the market-data grant still gets their FX rates and ADV volumes stored, but the
panel reports classification as unavailable rather than as a checked zero. See
[Access rights](access-rights.md).

## Reference data

This editor, shared across every portfolio, holds the per-instrument overrides that feed
concentration grouping, liquidity bucketing, and bond analytics everywhere else in the tool. It
lists every instrument currently held by any active portfolio (de-duplicated by ISIN/code across
the whole fleet), with these columns:

| Column | Meaning | Editable? |
| --- | --- | --- |
| Code | The instrument's ISIN or code. | No. |
| Name | The instrument's name. | No. |
| Type | Its asset type (equity, bond, fund, future, cash account, and so on). | No. |
| Issuer group | The group used for concentration checks — merge connected issuers by giving them the same group name. Shows as a placeholder the default group (the normalized instrument name, or the bank code for a cash/margin account) when no override is set. | Yes. |
| Days | Days-to-liquidate, used by the Liquidity view. Shows the effective asset-type default as a placeholder when blank. | Yes. |
| ADV 30d | The most recently stored 30-day average daily volume. | No — comes from the Bloomberg ADV feed. |
| ADV as-of | The date that volume figure was fetched; carries a "stale" badge once it is older than the portfolio's configured maximum age. | No. |
| Market place | The trading venue name, from the depositary feed. | No. |
| ADV eligible | A three-way choice: **derived** (the tool decides based on the instrument and its market place), **always**, or **never**. | Yes. |
| Coupon %, Maturity, Freq | Bond-only fields (blank/dashed for a non-bond row). Frequency is a dropdown: annual, semi-annual, quarterly, or monthly. | Yes, for bonds. |
| (actions) | **Save** (enabled once a field is edited) and **Reset** (clears every override back to the default, shown whenever any override or bond field is set). | — |

A row's Save button only submits the fields you actually touched; leaving a field untouched keeps
its current stored value.

**Where these values come from when you don't type them:**

- **Issuer group and days-to-liquidate** default from the instrument's own name/type (issuer group)
  or the portfolio's configured asset-type defaults (days) whenever no override is set.
- **ADV, market place, and (for CACEIS-sourced bonds) coupon/maturity/frequency** are maintained by
  the depositary feed and the Bloomberg panel and cannot be typed here at all — the form rejects
  those fields outright if sent.
- **Bond coupon, maturity and frequency are auto-parsed from the instrument's own name** on import,
  whenever those fields are still blank — this applies to any bond position with a parseable name,
  regardless of import source. A value already present (typed by hand, or restated by a CACEIS
  import) is never overwritten by this parse.
- Because a CACEIS-sourced bond has its coupon and maturity restated by the depositary feed on
  every import (see "What CACEIS gives you and what it doesn't" above), hand-editing those two
  fields for such a bond is effectively futile: the next import simply restores the depositary's
  own value. Coupon frequency and non-CACEIS bonds are not affected by this.

### Access rights

Viewing this table requires reference-data viewing rights, held globally; editing a row (Save or
Reset) requires reference-data configure rights, also global. See
[Access rights](access-rights.md).

## Settings

Below Import history, a Settings card holds the per-portfolio configuration that the risk pages
(VaR/ES, Risk, Limits) read from — described in full in those chapters. This section only covers
what the panel itself looks like and what you can do here; see the [VaR / ES](var-es.md),
[Risk](risk.md) and [Limits](limits.md) chapters for what each figure actually drives.

The top row of fields covers, among others: the risk-free rate, VaR confidence/horizon/window and
its UCITS limit, the short-drawdown day threshold, the redemption stress shock, the ADV
participation rate and stress factor, the liquidity horizon and settlement deadline, the maximum
ADV age before a volume figure is treated as stale, and the flow lookback window. Change any field
and click **Save** to apply every change on the card at once; a confirmation or error message
appears next to the button.

When at least 20 daily flow observations are available for the portfolio (they come exclusively
from `JOURSRLUX` files — see "The CACEIS CSV feed" above; the NAV Recap workbook carries no
subscription/redemption history, so a portfolio fed only from the workbook never accumulates
any), a line shows the observed worst 20-day net
outflow as a percentage of NAV, next to an **"Adopt as fixed shock"** button that copies that
observed figure straight into the redemption stress field above — a shortcut for calibrating the
stress assumption to what has actually happened, rather than typing a number by hand.

### Liquidity defaults

Below the main settings, a row of fields — one per asset type actually held somewhere in the fund —
sets the **default number of days to liquidate** for that asset type. This is the fallback used by
the Liquidity view on the Limits page for any position that has no per-instrument override set in
the [Reference data](#reference-data) editor above; the per-instrument override, when present,
always takes priority over this asset-type default.

### Access rights

Viewing this card requires reference-data viewing rights on the portfolio; saving changes requires
reference-data configure rights on the portfolio. See [Access rights](access-rights.md).

## Shareholder register

Further down the page, the Shareholder register card is a hand-maintained list of the fund's
largest investors, used by the Limits page's "Top 5 holders" redemption scenarios.

**This is maintained entirely by hand.** The depositary feed only carries share-class-level data,
never investor identities, so nothing in the tool populates or reconciles this register
automatically. Nothing checks the entries against the fund's actual outstanding shares either —
a stale or wrong register silently changes the Top-5 redemption scenarios on the Limits page,
beyond the as-of date shown there.

The table has one row per investor entry:

| Column | Meaning |
| --- | --- |
| Label | Free-text name/description of the holder. |
| % of NAV | The holder's share of the fund, as a percentage (0–100). |
| As-of | The date this figure is accurate to. |
| (action) | Remove — deletes the row from the draft immediately (not applied until you save). |

**To edit the register:**

1. Change any field directly in the table, or click **Add entry** to append a new blank row (today's
   date is filled in automatically), or **Remove** to drop a row.
2. Watch the **Total** figure at the bottom — the sum of every row's percentage. If it exceeds
   100%, it turns into a warning color and the **Save** button is disabled; you cannot save an
   over-100% register.
3. Click **Save** to replace the portfolio's entire register with what is currently in the table
   (this is a full replace, not a per-row update — every row present when you save is what ends up
   stored, including ones you didn't touch this session). A confirmation or error message appears
   below.

Switching to a different portfolio while you have unsaved edits discards the draft rather than
carrying it over to the newly selected fund — the register you see is always scoped to the
portfolio you are currently viewing.

### Access rights

Viewing the register requires shareholder-register viewing rights on the portfolio; saving requires
shareholder-register import rights on the portfolio (which also implies viewing). An archived
portfolio refuses a save the same way it refuses any other new data. See
[Access rights](access-rights.md).

## Portfolio snapshot

At the very bottom of the Data page, a read-only table shows the raw position snapshot for the
selected portfolio, mainly useful as a sanity check right after an import. A **Date** dropdown
picks which snapshot to view, defaulting to the most recent one; the table itself lists **Type**,
**ISIN**, **Name**, **Ccy**, **Qty**, **Price**, **Valuation €**, and **Weight**, one row per
position. There is nothing to edit here — it exists purely so you can eyeball what actually landed
after an upload without leaving the Data page.

Viewing this table requires positions-viewing rights on the portfolio; see
[Access rights](access-rights.md).

## What is portfolio-scoped and what is shared

Everything tied to a single fund's own history — its imports, NAV history, positions, dividends,
operations, CTD analytics, EMIR figures, settings, and shareholder register — is scoped to that one
portfolio and does not affect any other. Instrument reference data (classifications, issuer groups,
liquidity overrides, bond statics, Bloomberg FX rates) and futures contract specs are **shared**
across every portfolio, since the same instrument or contract means the same thing no matter which
fund holds it — an override or classification set while looking at one portfolio's holdings applies
immediately to every other portfolio that also holds the same instrument. The Bloomberg request
workbook and its FX cross-check likewise sweep every active portfolio in one pass rather than being
run per portfolio.
