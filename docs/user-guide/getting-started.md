# Getting started

Borobudur Risk is a risk-monitoring dashboard for a set of portfolios — UCITS
funds and mandates, each tracked independently. It is fed from periodic NAV
data (an Excel "NAV Recap" workbook, or a custodian's own CSV exports) and
turns that into the analytics a risk or middle-office team checks day to day:
performance and drawdowns, VaR / ES against the UCITS 99%/20-day monitoring
rule, concentration and liquidity limits, derivatives and EMIR exposure, and
a profit-and-loss breakdown reconciled to the fund's own net asset value. It
runs as a single web page in your browser. Depending on how your organization
has set it up, it runs either as a **desktop** application on your own
machine, or as a **server** shared by several people, each with their own
account and their own access rights.

This chapter explains how to start the application (or, in server mode, how
to sign in to one someone else started), what you see when you first arrive,
and how to move around the app. If a tab or a button described here does not
appear on your screen, the most likely reason is that your account does not
have the access right that would make it useful — see
[Access rights](access-rights.md) for how that works.

## Desktop mode vs. server mode

Borobudur Risk can run in two modes. They look almost identical once you are
signed in — the difference is entirely in how the application starts and how
you get into it.

- **Desktop mode** is a single-user installation running on your own
  computer. There is no login screen: the moment the application starts, you
  are already "signed in" as the one user of that machine, with full access
  to everything. This is the mode described in the "Desktop mode" section
  below.
- **Server mode** is a shared installation running on a server that several
  people connect to over the network, each through their own account with
  their own access rights. This is the mode described in "Server mode"
  below, starting from "Signing in".

Nothing in the application tells you outright which mode you are in. The
practical tell is the sidebar: in server mode you see your name and a
**Sign out** button at the bottom of the sidebar; in desktop mode that area
is empty, because there is no session to sign out of.

## Desktop mode

### Installing and starting the application

Desktop mode does not need to be "installed" in the traditional sense —
there is no installer, no admin rights, and nothing is written outside your
own user profile. Someone (an IT contact, or you, if you were given the
project) builds the application once from its source code, which produces a
single program file, `server.exe`, plus a companion `frontend` folder of
built web assets sitting next to it. From then on, starting the application
is a matter of running that one file.

1. Locate `server.exe` (and the `frontend` folder beside it) wherever it was
   placed on your machine — typically inside a `target\release` folder if
   you built it yourself, or a folder your IT contact gave you.
2. Double-click `server.exe`, or run it from a command prompt.
3. Wait for the first-start setup described below, if this is the first time
   the application has run on this machine.
4. Your default web browser opens automatically to the application. If it
   does not, open a browser yourself and go to `http://127.0.0.1:8787`.

There is no sign-in step in desktop mode. As soon as the page loads, you are
in the application.

### What happens the first time you start it

The very first time `server.exe` runs on a given machine, it needs a
database to store your portfolios, positions and analytics in. It downloads
and sets up a private copy of PostgreSQL for itself — you do not need
PostgreSQL already installed, and this copy is not shared with any other
application. Two things follow from this:

- **The first start needs a working internet connection**, to download the
  database engine. Every subsequent start reuses what was downloaded and
  works fully offline.
- **The first start takes noticeably longer** than normal — the browser
  window opens once this one-time setup has finished, so a blank few seconds
  before the page appears is expected on that first run only.

This private database is stored under your Windows user profile (inside
`%LOCALAPPDATA%\borobudur-risk`), separate from the application program
itself. It is what holds all your data between sessions — closing
`server.exe` does not delete anything; starting it again reopens the same
data.

### Loading your first data

A freshly started application has one built-in portfolio and no positions in
it yet. To get analytics on your screen:

1. Open the **Data** tab in the sidebar (it is described in the
   [Data](data.md) chapter in full).
2. Upload the periodic NAV Recap workbook for the fund you want to track, or,
   for a portfolio administered by CACEIS Bank Luxembourg, drop the
   custodian's own CSV exports there instead.
3. Once at least one snapshot has been imported, the Overview, Performance,
   Risk and other tabs populate with analytics for that portfolio.

Exactly what to upload, in what order, and how to handle the follow-on steps
(confirming new futures contracts, uploading the CTD companion file, running
the Bloomberg classification workbook) is covered in the [Data](data.md)
chapter.

## Server mode

In server mode, Borobudur Risk runs continuously on a server that your
organization controls; you connect to it with a browser over the network
(typically an internal address or a VPN, given to you by an administrator)
rather than starting it yourself. Everything past this point in this chapter
— getting an account, signing in, the layout — applies to server mode.
Desktop mode users can skip ahead to "Navigating the app", since the layout
itself is identical once you're in.

### Getting your account

You cannot create your own account. An administrator creates it for you on
the Administration page, and at that moment the application generates a
random password and shows it to the administrator exactly once — it is never
stored anywhere it can be read back, and it is never emailed to you by the
application. Your administrator hands it to you out of band (in person, or
over whatever secure channel your organization uses).

Two things follow from that:

- **There is no self-service password change or reset.** If you lose your
  password, or want a different one, ask your administrator to reset it —
  they receive a freshly generated password to hand to you, again shown to
  them only once. A reset also ends any session you had open, so you will
  need to sign in again with the new password everywhere.
- **There is no "forgot password" link on the sign-in screen** — the
  administrator reset above is the recovery path.

The one exception is the very first administrator on a brand-new
installation: no administrator exists yet to hand them a password, so the
server instead prints a single-use **enrolment token** to its own startup
log, valid for one hour, with which that first administrator sets their own
password. That bootstrap step is described in
[Administration](administration.md); every account after it uses the
generated-password flow above.

### Signing in

1. Open the address your administrator gave you in a web browser. If you are
   not signed in (or your session has expired — see "When your session
   expires" below), you land on the sign-in screen, titled **Borobudur
   Risk**.
2. Enter your **Email** in the first field.
3. Enter your **Password** in the second field.
4. Click **Sign in** (this button, and the two fields, are disabled while a
   sign-in attempt is in progress, and the button stays disabled until both
   fields have something in them).

If sign-in fails, a message appears above the button explaining why —
for example, wrong credentials, or that the browser could not reach the
server at all (which reads as a connection problem, distinct from a rejected
password). Wrong credentials do not tell you whether the email or the
password was the problem — the server treats an unrecognized email and a
wrong password identically, so that a failed attempt cannot be used to
probe which email addresses have accounts.

Repeated failed attempts on the same account temporarily lock it out: after
five wrong passwords in a row, further attempts are refused for the next
15 minutes, even if you then supply the correct password. If you find
yourself locked out, wait for the lockout to expire — attempts made while
locked are simply refused; they neither reset nor extend the timer, and an
administrator has no button to clear the lock early.

**If sign-in appears to do nothing at all** — the page does not show an
error, but you are never taken past the sign-in screen — the most likely
cause is that the connection to the server is not encrypted (plain HTTP
instead of HTTPS). The application only ever accepts its session cookie over
an encrypted connection; over plain HTTP the browser silently discards it,
so every sign-in looks like it "didn't take" with no error to explain why.
This is not something you can fix from the browser — contact your
administrator and report that sign-in is silently failing, since it usually
means the encrypted connection in front of the server is missing or
misconfigured.

## Navigating the app

Once you are signed in (or, in desktop mode, as soon as the application
opens), you see the main layout: a sidebar on the left and the current page's
content on the right.

### The sidebar

From top to bottom, the sidebar shows:

1. **Borobudur Risk** — the application name, at the top.
2. **The portfolio selector** — a dropdown listing every portfolio you have
   access to that has not been archived. Mandates are listed with a
   `(mandat)` suffix after their name so they are visually distinguishable
   from UCITS funds; ordinary funds show just their name. Choosing a
   different portfolio here switches the whole app to that portfolio,
   keeping you on the same kind of page (for example, switching portfolios
   while on the Risk tab keeps you on the Risk tab, now showing the newly
   selected portfolio).
3. **The page tabs**, one link per page: **Overview**, **Performance**,
   **P&L**, **Risk**, **VaR / ES**, **Limits**, **Derivatives**, and
   **Data**. Each is covered in its own chapter of this guide.
4. **Administration** — an extra link, shown only to administrators, that
   leaves the portfolio view entirely for the instance-wide administration
   screens (users, roles, grants, audit log). See
   [Administration](administration.md).
5. **Your name and a Sign out button**, at the very bottom — server mode
   only. Desktop mode has no session to sign out of, so this area is simply
   not shown.

### Tabs are hidden, not just disabled, when you lack the access right for them

Every page tab in the sidebar requires at least one access right (a "grant")
on the portfolio you are currently viewing; a tab you have no relevant grant
for is left out of the sidebar entirely rather than shown greyed out. This
means the set of tabs you see can differ from one portfolio to the next, and
from one colleague to the next, depending on what each of you has been
granted. Administration behaves the same way, but on a separate axis: it
only ever appears for accounts flagged as administrators, independent of
portfolio-level grants.

This hiding is a convenience, not a hard boundary — even where a tab is
shown, individual sections inside a page can still tell you they are
unavailable if you lack the narrower access right that section itself needs
(the Data page in particular combines several access rights at once, so it
stays visible as long as any one of them would make at least one panel on it
useful, and the rest of the page degrades panel by panel). The full model —
which access right controls which tab and which section — is described in
[Access rights](access-rights.md).

### The portfolio the app remembers

The application remembers the last portfolio you viewed (in your browser, on
that device) and returns you to it automatically the next time you arrive at
the application's root address. If that remembered portfolio is no longer
available to you — for example, it was archived, or your access to it was
revoked — you are sent to your first available active portfolio instead.

If you have no active portfolios to view at all, you land on a plain page
that says "No active portfolios yet" instead of the usual sidebar layout. If
at least one portfolio exists that you could be pointed toward (even an
archived one), a **Manage portfolios** link is offered to reach the Data
page for it. Administrators additionally get an **Administration** link
here, since this "no portfolios" screen is otherwise the one place in the
app an administrator with no personal portfolio access could get stuck with
no way to reach the administration screens at all.

### Signing out

Click **Sign out** at the bottom of the sidebar (server mode only — see
above). This ends your session both on the server and in your browser, and
returns you to the sign-in screen. Signing out always takes you back to the
sign-in screen even if the request to the server fails for some reason (for
example, if the connection drops at that exact moment) — the point of
clicking it is to stop being signed in on your own screen, so it does not
leave you stranded on a broken page waiting for a server response.

### When your session expires while you are working

A server-mode session is valid for 12 hours. If it expires
while you are in the middle of using the application — or if an
administrator revokes your session — the next action you take that needs the
server (loading a page, refreshing data, submitting something) drops you
back to the sign-in screen without warning beforehand. This is expected
behavior, not a bug: simply sign in again with your email and password. The
application takes you back to the exact page you were on before you were
signed out — it does not reset you to the Overview tab or to your default
portfolio, so you resume where you left off, with a fresh session, rather
than needing to navigate there again.

## Where to next

- [Overview](overview.md) — the portfolio summary page
- [Performance](performance.md) — returns, volatility, Sharpe, drawdowns
- [P&L](pnl.md) — profit-and-loss attribution and AUM reconciliation
- [Risk](risk.md) — risk metrics for the current portfolio
- [VaR / ES](var-es.md) — Value-at-Risk / Expected Shortfall and back-testing
- [Limits](limits.md) — UCITS concentration checks and liquidity bucketing
- [Derivatives / EMIR](derivatives-emir.md) — derivatives exposure and EMIR clearing thresholds
- [Data](data.md) — imports, reference data, and the weekly workflow
- [Administration](administration.md) — users, roles, grants, and the audit log (administrators only)
- [Access rights](access-rights.md) — how grants control what each tab and section shows
