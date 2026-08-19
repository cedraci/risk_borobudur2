# Access rights

Borobudur Risk controls who can see and do what through a permission model built from three
independent pieces: **what kind of data** (domain), **what you can do with it** (action), and
**which portfolios** it applies to (scope). A fourth, separate switch marks someone as an
**administrator** — the ability to manage users and permissions, which has nothing to do with
seeing fund data. This chapter explains all four, lists the ready-made roles, and shows exactly
what a denial looks like on screen.

In **desktop mode** (the tool running locally for a single user, with no login screen) none of
this applies: the desktop user automatically has every permission on every portfolio, including
administrator rights. Everything below describes **server mode**, where the tool is shared by
several people who sign in with their own account.

## The six domains

A domain is a category of data. Every permission you are given is for one domain at a time.

| Domain | What it covers |
| --- | --- |
| Positions | Portfolio holdings and everything computed from them: P&L, concentration, liquidity, rates/DV01, derivatives notionals, EMIR monitoring. |
| NAV | The fund's NAV history and everything computed from it: performance, drawdowns, VaR/ES and VaR back-testing, the Overview and Risk dashboards. |
| Transactions | The trade/dealing history used to split P&L into realized and unrealized amounts. |
| Shareholders | The shareholder register and subscription/redemption flows, used for redemption stress testing. |
| Market data | Market-sourced analytics such as bond futures' CTD (cheapest-to-deliver) data. |
| Reference | Static/reference data: instrument classifications (country, sector, industry, issuer group), portfolio settings, and the import log. |

## The four actions

Within a domain, a separate permission is needed for each of four actions:

| Action | Meaning |
| --- | --- |
| View | See the data and any figures computed from it. |
| Export | Download or export the data (for example the EMIR evidence export). |
| Import | Upload new data into that domain (for example NAV Recap or CACEIS files). |
| Configure | Change settings within that domain (for example editing portfolio settings or reference data by hand). |

**A permission to export, import, or configure a domain always includes the right to view it.**
You cannot be granted the ability to export or edit data you are not allowed to see — granting
one of those three actions automatically grants viewing as well. The reverse is not true: you can
very well be able to view a domain without being able to export, import, or configure it. In
practice this means, for example, that someone can see the Derivatives page but have the "export
evidence" button denied, while it is never possible to be able to export something you cannot
view.

## Portfolio scope

Every individual permission is either:

- **Scoped to one portfolio** — it applies only to that fund or mandate, or
- **All portfolios** — a "wildcard" permission that applies to every portfolio, including ones
  created later.

A person can hold a mix: for example, full access to one fund and only NAV-viewing rights on
another. There is no concept of "default" access — if nothing has been granted for a given
domain, action and portfolio, it is denied.

## Roles: ready-made permission bundles

Rather than granting domains and actions one at a time, an administrator can apply a **role** to a
user at a chosen scope (one portfolio, or all portfolios). Applying a role writes out its grants
immediately; it is a one-time bundle of permissions, not a live link, so changing what a role
means afterwards does not retroactively change people who were already given it — an
administrator has to re-apply it.

| Role | Grants |
| --- | --- |
| Risk Analyst | View and export on positions, NAV, transactions, market data, and reference data. (No shareholder register access, and no configure rights anywhere.) |
| Head of Risk | View and export on all six domains, plus the ability to configure reference data. The broadest analytical role. |
| Operations | Import rights on positions, NAV, transactions, and market data (which, as above, also grants viewing them), plus view-only access to reference data. Built for the people who load the weekly files rather than analyze the output. |
| Auditor | View-only access across all six domains. No export, import, or configure rights anywhere. |

Because grants can be freely added and removed after a role is applied, an administrator can also
fine-tune an individual's access beyond what a role provides — a role is a convenient starting
point, not a hard boundary.

## Administrator: a separate switch

Being an administrator is not a grant and is not tied to any domain. It is a single yes/no switch
on a user's account, separate from the six-domain permission model entirely. An administrator can:

- Manage users (create accounts and hand out their generated passwords, disable and re-enable
  them, reset passwords).
- Assign roles and edit individual grants for any user.
- View the audit log.

An administrator does **not** automatically see any fund data. If an administrator has not also
been granted view access to, say, positions on a given portfolio, the Administration page is still
fully available to them, but the portfolio's own pages (Overview, P&L, Limits, and so on) behave
exactly as they would for anyone else without that grant. Data access and administrative access
are independent: someone can be a full administrator with no data access at all, or have every
data permission on every portfolio without being an administrator.

See [Administration](administration.md) for the full walkthrough of the Administration page,
including how new users are enrolled and how the audit log is read.

## How a denial looks on screen

The tool never silently withholds something without saying so, and it never lets a lack of
permission masquerade as a real (empty, zero, or "all clear") result. Denials are always visible
and always labeled as denials, not as data.

- **A whole tab you have no use for is hidden.** The sidebar only lists the tabs for which you
  hold at least one relevant permission on the currently selected portfolio. If you have no
  permission that would make a tab useful, it simply does not appear — there is nothing to click
  through to find out it is empty. The Administration link only appears for administrators,
  following the separate switch described above.
- **Inside a page, a specific section you cannot see shows a grey "N/A" notice** rather than being
  left blank or showing a fake pass. Hover it (or read the text next to it) for the reason, which
  always starts with **"not permitted: "** followed by the domain that was denied — for example
  "not permitted: NAV history." This wording is deliberately different from a note saying data is
  simply absent (for example "no shareholder register data has been imported"), so you can always
  tell "I'm not allowed to see this" apart from "this genuinely doesn't exist yet."
- **A check or scenario that could not be evaluated because of a denial is marked unavailable, not
  passed.** For example, on the Limits page, if you cannot see reference data, the concentration
  checks that depend on issuer overrides show the same grey "N/A" treatment on every affected row
  and status chip — they are never shown in green as if the check had cleared.
- **Partial data is labeled as partial, not presented as complete.** Where a page draws on several
  domains and you are missing access to only some of them, the page still shows everything it can
  compute from what you do have access to, and calls out what it could not check with a warning or
  an "N/A" marker rather than either blocking the whole page or pretending the picture is
  complete. The P&L page is a good example: it needs positions to render at all, but transactions,
  reference data, market data and NAV each degrade independently — losing access to trade history,
  for instance, still lets the page show total P&L, just without the realized/unrealized split,
  flagged through a warning.
- **A portfolio outside your scope behaves as if it does not exist.** It never appears in the
  portfolio switcher, and if you have its address typed directly into the browser, the tool
  responds as though there were no such portfolio at all, rather than confirming that it exists
  but is off-limits. This is deliberate: a portfolio's very existence is not disclosed to someone
  with no permission on it at all. This is different from a permission denial on a portfolio you
  can otherwise see (through some other domain) — there, the tool does confirm the portfolio
  exists and tells you plainly which permission you are missing.

## If something looks wrong

If a tab, page, or section is missing or shows "N/A" and you believe you should have access,
contact your administrator — they can see and adjust your permissions from the Administration
page. Every grant made, role applied, and account change is written to the audit log, so changes
to access rights are always traceable after the fact. See [Administration](administration.md) for
where that log lives and what it records.
