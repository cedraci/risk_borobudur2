# Administration

The Administration screen exists only in server mode, and only administrators
see it — it is the extra link at the bottom of the sidebar described in
[Getting started](getting-started.md). It has nothing to do with any one
portfolio: it is where you manage the accounts of everyone who uses this
installation, decide what each of them can see and do, and read the
instance-wide log of who did what.

Being an administrator is a separate switch from having access to data. It
lets you reach this screen and act on it, but it does **not**, by itself, let
you see any portfolio's positions, NAV history, transactions or anything
else. An administrator who wants to look at portfolio data — for example, to
check that a grant they just set up actually works — needs their own grants
on that data, exactly like anyone else. Grants are covered below and in full
in [Access rights](access-rights.md); this chapter assumes you have read that
chapter's explanation of domains, actions and portfolio scope, and focuses on
how to work the controls on this screen.

## Users

The Users panel lists every account on the installation: display name,
email, whether they are an administrator, and whether they are currently
disabled.

### Creating a user

1. In the **Create user** form at the bottom of the Users panel, enter the
   new person's **Email** and **Display name**.
2. Tick **Administrator** if this person should also be an administrator.
   Decide this carefully: once the account is created there is no button on
   this screen to promote an ordinary user to administrator later, or to
   demote an administrator back to an ordinary user. If you get this wrong,
   the only way to fix it is to create a fresh account with the right flag
   and disable the old one.
3. Click **Create**.

The application immediately generates a random password for the new account
and shows it once, in a "Generated password" card, alongside the person's
email. This is the account's real password from the moment it is created —
there is no separate enrolment link or token to complete afterwards, and
there is no default password to fall back on if you lose this one. Copy it
now and hand it to the new user out of band (in person, by phone, or through
whatever secure channel your organization uses — not by leaving it in an
email that sits in an inbox indefinitely). Once you dismiss the card, or
navigate away from the page, that password is gone for good: the server
never stores it in a form it can show you again, only the hashed form it
checks logins against.

The new user signs in with this email and password exactly as described in
[Getting started](getting-started.md#signing-in).

### Re-issuing a password

If a generated password is lost before it reaches the user, or you simply
want to force a fresh password onto an account (for example, after a
suspected leak), use **Reset password** on that user's row:

1. Find the user in the Users table and click **Reset password**.
2. A new random password is generated and shown once, the same way as at
   creation. Copy it and relay it to the user out of band.

Resetting a password immediately ends every session currently open on that
account, on every device — the old password (and any cookie obtained under
it) stops working the instant the reset completes, and the user must sign in
again with the new password.

You cannot reset your own password from this screen. The button is disabled
on your own row, because doing so would end your own session before you had
a chance to see and copy the newly generated password — you would be signed
out and locked out of your own account in the same click. There is no
self-service password change anywhere in the application either: if you want
your own password changed, ask another administrator to reset it for you.

### Disabling and re-enabling a user

Click **Disable** on a user's row to prevent that account from signing in.
Click it again (it becomes **Enable**) to restore access.

Disabling a user immediately ends every session currently open on that
account, on every device, the same as a password reset does — a disabled
user is signed out at once, not merely blocked from signing in again. This
is also, in practice, the way to force someone out of the application right
now without changing their password: disable the account, then immediately
re-enable it. Re-enabling does not restore the sessions that were just
closed; the person simply signs in again with their existing password.

**You cannot disable your own account.** As with password resets, the
button is disabled on your own row, since it would end your session
immediately.

**You cannot disable the last usable administrator.** If you try to disable
an administrator and no other administrator on the installation could
currently sign in, the action is refused with an error explaining that you
cannot disable the last administrator. "Could currently sign in" excludes
two kinds of administrator from counting as a fallback: anyone already
disabled, and anyone who has never completed the very first sign-in step for
their account (see "The very first administrator" below) — such an account
exists in the list and is flagged as an administrator, but cannot actually
authenticate yet, so it does not count as a safety net. This rail exists
purely to stop the installation from ending up with no one able to
administer it; it does not stop you from disabling an administrator as long
as at least one other one remains able to sign in.

## Permissions

Selecting **Permissions** on a user's row opens a panel with two ways to set
what that person can do: a grant matrix you edit directly, and a role you can
apply as a shortcut. Both write to the same underlying grants, so what you
see in the matrix always reflects the current, true state, however it got
there.

### Editing individual grants

The grant matrix lists the six data domains (positions, NAV history,
transactions, shareholder register, market data, reference data) as rows,
and the four actions (view, export, import, configure) as columns, for one
portfolio scope at a time.

1. Use the **Scope** dropdown above the matrix to choose which portfolio you
   are editing grants for, or leave it on **All portfolios** to grant access
   across every portfolio at once (including ones created later). A grant
   made under **All portfolios** is a different, broader thing from the same
   grant made under one named portfolio — see
   [Access rights](access-rights.md) for how scope is evaluated.
2. Tick or clear a checkbox to add or remove that domain/action grant at the
   currently selected scope. Each change is saved immediately — there is no
   separate "save" step, and no way to batch several changes before they
   take effect.
3. The **view** column is auto-managed: ticking export, import or configure
   for a domain automatically implies view for that same domain and scope,
   so the view checkbox shows as ticked and greyed out (disabled) whenever
   any of the other three is granted. To remove view entirely, remove
   export, import and configure for that domain and scope first.

If the portfolio list fails to load, or your own account has no portfolios
visible to it, a note appears under the scope selector explaining which of
the two happened, since both would otherwise look like nothing more than an
empty dropdown.

If an error appears when adding a grant — most often "no such portfolio" —
the portfolio you had selected in the scope dropdown was deleted by someone
else while your page was open. Refresh the page to get a current portfolio
list and try again.

### Assigning a role

Roles are a shortcut for applying a standard bundle of grants in one step,
rather than ticking every checkbox by hand. The four built-in roles are:

| Role | What it grants |
| --- | --- |
| Risk Analyst | View and export on positions, NAV history, transactions, market data, reference data and portfolio settings. No access to the shareholder register. |
| Head of Risk | View and export on all seven domains, plus configure on reference data and on portfolio settings. |
| Operations | Import on positions, NAV history, transactions and market data, plus view on reference data and portfolio settings. No shareholder register access, no export. |
| Auditor | View only, on all seven domains. No export, import or configure anywhere. |

To apply one:

1. Choose the **Role** and the **Scope** (a single portfolio, or **All
   portfolios**) below the grant matrix.
2. Click **Apply**.

Applying a role writes that bundle of grants to the user immediately, at the
chosen scope — it is a one-time action, not a live assignment. Two things
follow from that:

- **It is additive.** Applying a role adds its grants on top of whatever the
  user already has; it does not remove grants from a previously applied role
  or from individual edits. If you are moving someone from one role to a
  narrower one, you still need to remove the grants the old role left behind
  yourself, using the matrix above — there is no "unassign role" control
  that undoes what a role added.
- **It does not stay in sync.** Nothing remembers that this user "has" the
  Head of Risk role in a way that would update automatically if the role's
  definition changed later. Re-applying is the only way to bring a user's
  grants up to date with a role's current bundle.

The grant matrix above updates immediately after a successful Apply, so you
can confirm exactly what was written.

## Signing in and account lockout

Ordinary sign-in, session expiry and what a locked-out user sees are covered
in [Getting started](getting-started.md#signing-in). The exact lockout rule,
relevant to administrators troubleshooting a user who says they can't get
in, is this:

- After **five wrong passwords in a row on the same account**, that account
  is locked for **15 minutes**. During that window, every sign-in attempt on
  that account is refused outright — including one with the correct
  password — without even checking it.
- The lock is tied to the email address, not to any flag on the user record.
  **Disabling and re-enabling the account, or resetting its password, does
  not lift an active lockout.** There is no button anywhere in the
  application, including here, to clear a lockout early — the only way past
  it is to wait out the 15 minutes.
- Once the 15 minutes pass, the next wrong password immediately re-locks the
  account for another full 15 minutes. Only a successful sign-in clears the
  failure count back to zero. An account that keeps having wrong passwords
  entered against it can therefore stay locked out indefinitely, one
  15-minute window after another.

Separately from the per-account rule, the server also throttles **where the
attempts come from**. Account lockout only ever counts failures against one
email, so it does nothing about someone trying a single likely password
against hundreds of accounts in turn — no account ever reaches five. The
per-origin rule closes that:

- After **ten failed sign-ins from the same network address within 15
  minutes**, that address is told to wait, whatever accounts the attempts
  were aimed at. The wait starts at 30 seconds and doubles with each further
  failure, up to 15 minutes.
- A successful sign-in from that address clears its count immediately, so a
  colleague who mistypes their password a few times and then gets it right
  is never affected. Ten is also well above the five that lock a single
  account, so ordinary fumbling never reaches it.
- The address comes from the reverse proxy in front of the server. If the
  deployment has none, or does not forward the client address, the
  throttling has nothing to key on and only the per-account rule applies.
- These counts live in the server's memory, not the database, so restarting
  the server clears them. Account lockout, which is stored, does not clear.

A throttled attempt is recorded in the audit log as its own event, so a burst
of them from one address is visible after the fact.

## Revoking sessions

There is no "sessions" list to browse or a single button labelled "revoke
session" on this screen. Ending a user's active sessions is instead a side
effect of two actions covered above:

- **Reset password** ends every session on that account immediately, on
  every device, and the user must sign in again with the new password.
- **Disable** ends every session on that account immediately, on every
  device. If you want to force the person out right now without changing
  their password, disable the account and then immediately re-enable it —
  the closed sessions are not restored by re-enabling, so the person simply
  signs in again with their existing password.

## The very first administrator

There is one enrolment path that does not go through this screen at all:
standing up a brand-new installation with no users yet. Whoever manages the
server sets the first administrator's email in the server's own
configuration; on the next start, the server creates that administrator's
account and prints a one-hour, single-use token to its own startup log
rather than to this page. Using that token is what sets the account's first
real password: the token expires one hour after the server printed it, and
it is consumed the moment it is used — it cannot be reused even inside the
hour. There is no page in the application to redeem it through; whoever
manages the server submits it, together with the chosen password, as a
direct request to the server.

There is nothing on this screen to re-issue that token if it expires
unused, and — because the account already exists at that point — the server
will not generate a new one on a later restart either. If your organization's
very first administrator token expires before anyone uses it, contact
whoever manages the server rather than looking for a retry option here; the
account and its token cannot be regenerated from within the application.

Every user created from the Users panel above, including additional
administrators, is enrolled the ordinary way described in "Creating a user"
— a generated password shown once — never through this one-hour token,
which exists solely to bootstrap the very first account on an empty
installation.

## The audit log

The Audit log panel at the bottom of the page is a read-only, newest-first
list of the last 200 recorded events on the installation. There is no
control to see further back, and no delete or export button — it is exactly
what the server has logged, nothing more.

Each row shows:

| Column | What it shows |
| --- | --- |
| Time | When the event happened, in your local time. |
| Actor | The display name of whoever did it — or, for a failed or locked-out sign-in attempt that never resolved to a real account, the email address that was typed. |
| Action | A short label for what happened (see the categories below). |
| Domain | The data domain involved, if the event concerns one — a dash otherwise, for events like a login or a user being created that aren't about any one domain. |
| Portfolio | The portfolio the event concerns, if any — a dash for events that apply instance-wide (for example, most administration actions and every sign-in event). |
| Detail | The raw supporting detail for the event — which fields appear depends on the action. |
| Source | The network address the request came from, as reported by the reverse proxy in front of the server. A dash where the deployment could not attribute one (in particular, everywhere in the single-user desktop application). |

There is no search box, no date-range picker, and no filter by user, action
or domain on this screen — it is the flat, most-recent-200 list described
above, sorted newest first.

The events recorded fall into a handful of categories:

- **Imports** — uploading a NAV Recap workbook, a CACEIS CSV export, CTD
  analytics, a Bloomberg classification workbook, or the shareholder
  register.
- **Exports** — downloading data out of the application, such as the EMIR
  evidence export or a Bloomberg classification results file.
- **Configuration changes** — edits to reference data, portfolio setup,
  futures contract confirmation, EMIR KPI entries, and similar settings.
- **Authentication events** — a successful sign-in, a failed sign-in
  attempt, an account being locked out after too many failed attempts, and
  an attempt refused because its origin was being throttled. These carry the
  source address, which is usually the first thing worth knowing about a run
  of failures.
- **Administration actions** — everything on this page: a user created, a
  password reset, a user disabled or re-enabled, a grant added or removed,
  a role assigned, and the very first administrator completing enrolment.

Because imports, exports and configuration changes are also tied to a
domain and (usually) a portfolio, you can read the log alongside the Domain
and Portfolio columns to answer "who touched this portfolio's reference
data" or "who exported EMIR evidence for this fund" without needing any
filter control — with only 200 rows shown, scanning the table is normally
enough.
