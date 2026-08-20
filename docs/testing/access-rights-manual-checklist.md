# Access rights — manual UI checklist

Everything about access rights that a machine can check is already checked
elsewhere; **do not re-test it by hand**:

- `cargo test -p server` — the in-process contract: the full route/grant
  matrix (401/403/404), portfolio isolation, partial-denial markers on
  populated data, sessions, lockout, enrolment, audit.
- `scripts/test-access-rights.ps1` — the same contract re-proven end-to-end
  against a live server-mode instance, plus account lifecycle and the audit
  trail.
- `cd frontend && npm test` — the browser half: that every `unavailable`
  marker the server emits is actually rendered, that a denial never takes a
  pass or a breach colour, and which nav links each grant set unlocks. Added
  2026-08-20; before it, the UI side of feature wrapping was checked only by
  the eyes below, which is how four unread markers survived.

This checklist is the remainder: what only a person looking at the browser
can observe (what is *shown*, *hidden*, *greyed*, *worded*). About 25
minutes.

## Setup (5 min)

- [ ] Start a scratch instance: `cargo run -p server --example dev_server`
      (or use a staging deployment).
- [ ] Bootstrap and provision fixtures in one step:
      `pwsh scripts/test-access-rights.ps1 -EnrolToken <printed token> -AdminEmail admin@dev.local -AdminPassword <pick one> -KeepUiFixtures`
- [ ] Note the credentials the script prints: **administrator**, **subject**
      (nav + positions view on portfolio A only), portfolios **A** (ucits)
      and **B** (mandate), and the **locked** user.
- [ ] Window 1 (normal): sign in as the administrator, open Administration.
- [ ] Window 2 (private/incognito): use for the subject.

> The subject's checks below say "refresh" — grants apply server-side
> immediately; refreshing only re-renders the sidebar.

## 1. Sign-in screen (window 2, ~3 min)

- [ ] No "forgot password" link anywhere on the sign-in screen.
- [ ] Wrong password for a real email and any password for a nonexistent
      email produce the **same** error wording (no account probing).
- [ ] While a sign-in attempt is in flight, both fields and the button are
      disabled; the button stays disabled while either field is empty.
- [ ] Sign in as the **locked** user with its correct-looking password: the
      error clearly indicates a temporary lockout, not "wrong credentials".

## 2. Tab and selector gating (subject, ~5 min)

- [ ] Sign in as the subject. The portfolio selector lists **A only** — B
      does not exist as far as this account can see.
- [ ] Sidebar shows no **Administration** link.
- [ ] Tabs requiring domains the subject lacks are **absent entirely**, not
      greyed out (with nav+positions view: no import-dependent panels; exact
      tab set depends on grants — the point is *hidden vs greyed*).
- [ ] Admin (window 1): in the subject's grant matrix, remove
      **positions/view** on A. Subject refreshes: P&L, Risk, Limits and
      Derivatives disappear from the sidebar; Overview/Performance/VaR
      remain (nav is still granted). Re-add positions/view afterwards.
- [ ] Admin: add **positions/view** under scope **All portfolios**. Subject
      refreshes: **B appears in the selector with the "(mandat)" suffix**
      (A, a ucits, shows no suffix). Remove the wildcard grant again;
      B vanishes from the selector on the next refresh.
- [ ] Direct-URL probe: as the subject, paste the URL of a page for
      portfolio B (copy it from the admin window). The app behaves as if
      the portfolio does not exist — no name, no data, no distinguishable
      "forbidden" state.

## 3. Grant matrix UI (admin, ~4 min)

- [ ] Ticking **export** on a domain auto-ticks **view** and greys the view
      checkbox (disabled). Unticking export frees view again.
- [ ] Every tick/untick saves immediately — there is no Save button, and a
      refresh shows the same state.
- [ ] The **Scope** dropdown switches the matrix between All portfolios and
      each named portfolio; the same domain/action shows independent state
      per scope.
- [ ] Applying a **role** updates the matrix on screen immediately, and
      applying a second role adds to (never replaces) what is there.
- [ ] The four roles' bundles match the documentation
      (docs/user-guide/access-rights.md): Auditor is view-only everywhere;
      Risk Analyst has no shareholder register access; Operations has
      import but no export; Head of Risk adds reference/configure and
      settings/configure. The matrix lists seven domains, with
      **Portfolio settings** distinct from **Reference data**.

## 4. Denial wording on populated data (~5 min)

These need at least one imported snapshot. As the administrator: grant
yourself import rights on A, open the Data tab, and upload a NAV Recap
workbook (any recent one). Then, as the **subject** (nav + positions view
on A only):

- [ ] VaR / ES tab works but shows the settings-unavailable notice and
      falls back to the standard 99% / 20-day / 252-day defaults (no
      reference/view).
- [ ] Limits → liquidity: the top-5 redemption scenario reads
      **"not permitted: shareholder register"** — visibly different from
      the "no shareholder register" wording the administrator sees on a
      fund whose register was simply never uploaded. The ADV-coverage line
      above it says the same thing rather than reporting an empty register.
- [ ] Denied sections read **N/A / unavailable — never PASS/OK, and never
      BREACH either**: a missing grant is not a finding, and must not take
      the red used for one anywhere on Limits/Derivatives.
- [ ] Derivatives / EMIR: clearing-obligation verdicts show a grey
      **N/A** with the reason named above the table (reference denied), and
      the evidence export button produces an explicit error — never a
      silently degraded file.
- [ ] P&L: the **Realized and Unrealized columns are blank** (dashes), with
      the reason named above the table. Total and "of which FX" still show
      real figures. A realized column full of `0 €` is the specific failure
      this is looking for.

## 5. Account lifecycle UX (admin, ~4 min)

- [ ] Create a throwaway user: the **generated password card** appears
      exactly once, with the email beside it; after dismissing or
      navigating away it cannot be recovered anywhere.
- [ ] **Reset password** on another user shows the same one-time card.
- [ ] On **your own row**, Reset password and Disable are disabled.
- [ ] Trying to disable the last usable administrator produces a readable
      error (the script already proved the 422; check the UI surfaces it).
- [ ] The audit log table renders the script's run: user_created,
      grant_added/removed, role_assigned, logins — and the failed/locked
      sign-ins show the **typed email** as the actor.
- [ ] Every row carries a **Source** address. On a dev server this is
      `127.0.0.1`; behind a proxy it is whatever the proxy forwards. A column
      of dashes means the deployment is not forwarding the client address —
      see the server-mode section of the README.

## 6. Session UX (both windows, ~4 min)

- [ ] Subject is browsing some tab. Admin resets the subject's password.
      Subject's **next click** drops them to the sign-in screen with no
      prior warning.
- [ ] Subject signs back in (new password): they land on the **same page
      and portfolio** they were on, not on a default.
- [ ] **Sign out** returns to the sign-in screen; the name + Sign out
      block sits at the sidebar's bottom (server mode only).
- [ ] Remove **all** of the subject's grants, then refresh as subject:
      the **"No active portfolios yet"** page appears (no sidebar).
- [ ] Give the administrator account itself zero grants and reload: data
      tabs are gone but the no-portfolios page still offers the
      **Administration** link (an admin can never be stranded).

## 7. Deployment-only (staging with TLS in front, ~2 min)

- [ ] Over **https**, sign-in works normally.
- [ ] Over **plain http** to the same server-mode instance (not
      127.0.0.1), sign-in silently "does nothing" — no error, never past
      the sign-in screen — because the browser drops the `Secure` session
      cookie. This is the documented tell for a missing TLS terminator.
      (On 127.0.0.1 browsers treat http as trustworthy, so the dev server
      is exempt — test this only on a real hostname.)

---

Done: `PASS` from the script + all boxes above ticked = the access-rights
feature is verified end to end.
