#Requires -Version 7
<#
.SYNOPSIS
    Automated end-to-end test of the access-rights feature against a LIVE
    server-mode instance, over plain HTTP like any client.

.DESCRIPTION
    Covers everything about access rights that does not need eyes on the UI:
    authentication, the admin-vs-data axis, portfolio scoping (404 for
    out-of-scope, 403 for wrong domain), the export/import/configure -> view
    implication, wildcard (all-portfolios) grants, role bundles and their
    additivity, live revocation, session revocation on disable/reset, the
    lockout rule, the last-administrator guard, enrolment-token hygiene and
    the audit trail.

    The in-process Rust suite (cargo test -p server) already pins the full
    route/grant matrix; this script re-proves the same contract against the
    deployed binary end to end, then (with -KeepUiFixtures) leaves behind the
    users and portfolios the manual UI checklist needs
    (docs/testing/access-rights-manual-checklist.md).

    Target: any server-mode instance. For a local throwaway one:
        cargo run -p server --example dev_server
    then copy the enrolment token it prints:
        pwsh scripts/test-access-rights.ps1 -EnrolToken <token> `
            -AdminEmail admin@dev.local -AdminPassword <choose one>

    Against an already-bootstrapped instance, pass -AdminEmail/-AdminPassword
    of an existing administrator and omit -EnrolToken.

    All users and portfolios it creates carry a per-run tag, so re-running
    never collides. Without -KeepUiFixtures everything it created is disabled
    or archived at the end (accounts cannot be deleted, by design).

.PARAMETER BaseUrl
    Root URL of the instance (default http://127.0.0.1:8788, the dev_server
    example's bind).

.PARAMETER EnrolToken
    First-administrator enrolment token (fresh instance only). When given,
    the script enrols AdminEmail with AdminPassword before logging in.

.PARAMETER KeepUiFixtures
    Keep (and print) the subject user, portfolios and grants the manual UI
    checklist starts from, instead of cleaning everything up.

.PARAMETER SkipLockout
    Skip the lockout test (it deliberately locks a throwaway account for 15
    minutes and writes login_failed/login_locked audit rows).
#>
[CmdletBinding()]
param(
    [string]$BaseUrl = 'http://127.0.0.1:8788',
    [Parameter(Mandatory)][string]$AdminEmail,
    [Parameter(Mandatory)][string]$AdminPassword,
    [string]$EnrolToken,
    [switch]$KeepUiFixtures,
    [switch]$SkipLockout
)

$ErrorActionPreference = 'Stop'
$BaseUrl = $BaseUrl.TrimEnd('/')

# ---------------------------------------------------------------- plumbing

$script:Pass = 0; $script:Fail = 0; $script:Skipped = 0
$script:Failures = [System.Collections.Generic.List[string]]::new()

function Check([string]$Name, [bool]$Ok, [string]$Detail = '') {
    if ($Ok) {
        $script:Pass++
        Write-Host "  [PASS] $Name" -ForegroundColor Green
    } else {
        $script:Fail++
        $script:Failures.Add("$Name  $Detail")
        Write-Host "  [FAIL] $Name  $Detail" -ForegroundColor Red
    }
}
function Skip([string]$Name, [string]$Why) {
    $script:Skipped++
    Write-Host "  [SKIP] $Name  ($Why)" -ForegroundColor Yellow
}
function Phase([string]$Title) { Write-Host "`n== $Title" -ForegroundColor Cyan }

# The server-mode cookie carries `Secure`, which .NET's cookie container
# refuses to replay over http — so cookies are handled by hand: captured
# from Set-Cookie, sent back as a literal Cookie header.
function Invoke-Api {
    param(
        [string]$Method = 'GET',
        [Parameter(Mandatory)][string]$Path,
        [string]$Cookie,
        $Body
    )
    $p = @{
        Method             = $Method
        Uri                = "$BaseUrl$Path"
        SkipHttpErrorCheck = $true
        Headers            = @{}
    }
    if ($Cookie) { $p.Headers['Cookie'] = $Cookie }
    if ($null -ne $Body) {
        $p.ContentType = 'application/json'
        $p.Body = $Body | ConvertTo-Json -Depth 8
    }
    $r = Invoke-WebRequest @p
    $json = $null
    if ($r.Content -is [string] -and $r.Content.Length -gt 0) {
        try { $json = $r.Content | ConvertFrom-Json } catch { }
    }
    [pscustomobject]@{
        Status  = [int]$r.StatusCode
        Json    = $json
        Raw     = if ($r.Content -is [string]) { $r.Content } else { '' }
        Headers = $r.Headers
    }
}

function Get-SessionCookie([string]$Email, [string]$Password) {
    $r = Invoke-WebRequest -Method POST -Uri "$BaseUrl/api/login" -SkipHttpErrorCheck `
        -ContentType 'application/json' `
        -Body (@{ email = $Email; password = $Password } | ConvertTo-Json)
    $cookie = $null
    foreach ($sc in @($r.Headers['Set-Cookie'])) {
        if ($sc -match 'borobudur_session=([^;]+)') { $cookie = "borobudur_session=$($Matches[1])" }
    }
    [pscustomobject]@{ Status = [int]$r.StatusCode; Cookie = $cookie }
}

function New-Grant([string]$Domain, [string]$Action, $Portfolio) {
    @{ domain = $Domain; action = $Action; portfolio = $Portfolio }
}

function New-Password { -join ((65..90) + (97..122) + (48..57) | Get-Random -Count 20 | ForEach-Object { [char]$_ }) }

$tag = Get-Date -Format 'MMddHHmmss'
Write-Host "access-rights E2E against $BaseUrl (run tag $tag)"

# ------------------------------------------------- phase 0: reachability

Phase 'Reachability and bootstrap'
try { $health = Invoke-Api -Path '/api/health' }
catch { Write-Host "cannot reach $BaseUrl : $_" -ForegroundColor Red; exit 2 }
Check 'GET /api/health is 200' ($health.Status -eq 200) "got $($health.Status)"

# A desktop-mode instance has no accounts at all — nothing here applies.
$probe = Invoke-Api -Method POST -Path '/api/login' -Body @{ email = 'x'; password = 'x' }
if ($probe.Status -eq 404) {
    Write-Host 'this instance runs in DESKTOP mode (no login) — start a server-mode one, e.g.:' -ForegroundColor Red
    Write-Host '    cargo run -p server --example dev_server' -ForegroundColor Red
    exit 2
}

if ($EnrolToken) {
    $r = Invoke-Api -Method POST -Path '/api/enrol' -Body @{ token = $EnrolToken; password = $AdminPassword }
    Check 'enrolment with the startup token succeeds (204)' ($r.Status -eq 204) "got $($r.Status) $($r.Raw)"
    $r = Invoke-Api -Method POST -Path '/api/enrol' -Body @{ token = $EnrolToken; password = 'replayed' }
    Check 'the token is single-use: replay is 401' ($r.Status -eq 401) "got $($r.Status)"
}

$admin = Get-SessionCookie $AdminEmail $AdminPassword
Check 'administrator can sign in' ($admin.Status -eq 200 -and $admin.Cookie) "status $($admin.Status)"
if (-not $admin.Cookie) {
    Write-Host 'cannot continue without an administrator session' -ForegroundColor Red
    exit 2
}
$adm = $admin.Cookie
$me = Invoke-Api -Path '/api/me' -Cookie $adm
Check '/api/me reports is_administrator=true' ($me.Json.is_administrator -eq $true)

# ------------------------------------------- phase 1: unauthenticated wall

Phase 'Unauthenticated requests are refused'
foreach ($path in '/api/me', '/api/portfolios', '/api/admin/users', '/api/portfolios/1/nav', '/api/refs') {
    $r = Invoke-Api -Path $path
    Check "GET $path without a cookie is 401" ($r.Status -eq 401) "got $($r.Status)"
}
$r = Invoke-Api -Method POST -Path '/api/enrol' -Body @{ token = 'no-such-token'; password = 'x' }
Check 'enrolment with a garbage token is 401' ($r.Status -eq 401) "got $($r.Status)"
$r = Invoke-Api -Path '/api/me' -Cookie 'borobudur_session=forged-cookie-value'
Check 'a forged session cookie is 401' ($r.Status -eq 401) "got $($r.Status)"

# ---------------------- phase 2: administrator flag is not a data grant

Phase 'Administrator flag vs data grants (separate axes)'
$users = (Invoke-Api -Path '/api/admin/users' -Cookie $adm).Json
$adminRow = $users | Where-Object email -eq $AdminEmail
Check 'administrator appears in the users list' ($null -ne $adminRow)
$adminId = $adminRow.id

$capabilities = @($me.Json.capabilities)
$hadGlobalRefConfigure = [bool]($capabilities | Where-Object {
        $_.domain -eq 'reference' -and $_.action -eq 'configure' -and $null -eq $_.portfolio_id })
if (-not $hadGlobalRefConfigure) {
    $r = Invoke-Api -Method POST -Path '/api/portfolios' -Cookie $adm -Body @{ name = "never-created-$tag"; kind = 'ucits' }
    Check 'an administrator WITHOUT grants cannot create a portfolio (403)' ($r.Status -eq 403) "got $($r.Status)"
    $r = Invoke-Api -Method POST -Path "/api/admin/users/$adminId/grants" -Cookie $adm `
        -Body (New-Grant 'reference' 'configure' $null)
    Check 'administrator self-grants reference/configure (all portfolios)' ($r.Status -eq 204) "got $($r.Status)"
} else {
    Skip 'admin-without-grants 403 probe' 'this administrator already holds reference/configure globally'
}

$r = Invoke-Api -Method POST -Path '/api/portfolios' -Cookie $adm -Body @{ name = "E2E-A $tag"; kind = 'ucits' }
Check 'portfolio A created' ($r.Status -eq 200 -and $r.Json.id) "got $($r.Status) $($r.Raw)"
$pidA = $r.Json.id
$r = Invoke-Api -Method POST -Path '/api/portfolios' -Cookie $adm -Body @{ name = "E2E-B $tag"; kind = 'mandate' }
Check 'portfolio B (mandate) created' ($r.Status -eq 200 -and $r.Json.id) "got $($r.Status) $($r.Raw)"
$pidB = $r.Json.id
$r = Invoke-Api -Method POST -Path '/api/portfolios' -Cookie $adm -Body @{ name = "E2E-A $tag"; kind = 'ucits' }
Check 'a duplicate portfolio name is 422' ($r.Status -eq 422) "got $($r.Status)"

# ------------------------------------- phase 3: subject user, zero grants

Phase 'Subject user with zero grants'
$subjectEmail = "e2e-subject-$tag@test.local"
$subjectPw = New-Password
$r = Invoke-Api -Method POST -Path '/api/admin/users' -Cookie $adm `
    -Body @{ email = $subjectEmail; display_name = "E2E Subject $tag"; password = $subjectPw; is_administrator = $false }
Check 'subject user created' ($r.Status -eq 200 -and $r.Json.id) "got $($r.Status) $($r.Raw)"
$subjectId = $r.Json.id
$r = Invoke-Api -Method POST -Path '/api/admin/users' -Cookie $adm `
    -Body @{ email = $subjectEmail; display_name = 'dup'; password = 'x'; is_administrator = $false }
Check 'creating the same email again is 422' ($r.Status -eq 422) "got $($r.Status)"

$s = Get-SessionCookie $subjectEmail $subjectPw
Check 'subject can sign in' ($s.Status -eq 200 -and $s.Cookie) "status $($s.Status)"
$sub = $s.Cookie
$me = Invoke-Api -Path '/api/me' -Cookie $sub
Check 'subject is not an administrator and has zero capabilities' `
    ($me.Json.is_administrator -eq $false -and @($me.Json.capabilities).Count -eq 0)
$list = Invoke-Api -Path '/api/portfolios' -Cookie $sub
$ids = @($list.Json | ForEach-Object id)
Check 'portfolio list shows neither A nor B' ($ids -notcontains $pidA -and $ids -notcontains $pidB) "saw $($ids -join ',')"
$r = Invoke-Api -Path "/api/portfolios/$pidA/nav" -Cookie $sub
Check 'portfolio A reads as nonexistent (404), not merely forbidden' ($r.Status -eq 404) "got $($r.Status)"
$r = Invoke-Api -Path '/api/admin/users' -Cookie $sub
Check 'admin endpoints are 403 for a non-administrator' ($r.Status -eq 403) "got $($r.Status)"
$r = Invoke-Api -Method PUT -Path "/api/admin/users/$adminId/disabled" -Cookie $sub -Body @{ disabled = $true }
Check 'a non-administrator cannot disable anyone (403)' ($r.Status -eq 403) "got $($r.Status)"
$r = Invoke-Api -Method POST -Path '/api/portfolios' -Cookie $sub -Body @{ name = "never-$tag"; kind = 'ucits' }
Check 'a non-administrator without grants cannot create portfolios (403)' ($r.Status -eq 403) "got $($r.Status)"

# --------------------- phase 4: one scoped grant — right/wrong/elsewhere

Phase 'Portfolio-scoped grant: right domain 200, wrong domain 403, other portfolio 404'
$r = Invoke-Api -Method POST -Path "/api/admin/users/$subjectId/grants" -Cookie $adm `
    -Body (New-Grant 'nav' 'view' $pidA)
Check 'grant nav/view on A added' ($r.Status -eq 204) "got $($r.Status)"
$r = Invoke-Api -Path "/api/portfolios/$pidA/nav" -Cookie $sub
Check 'A/nav is now 200 — no re-login needed' ($r.Status -eq 200) "got $($r.Status)"
$r = Invoke-Api -Path "/api/portfolios/$pidB/nav" -Cookie $sub
Check 'B/nav stays 404 (grant is scoped to A)' ($r.Status -eq 404) "got $($r.Status)"
$r = Invoke-Api -Path "/api/portfolios/$pidA/positions" -Cookie $sub
Check 'A/positions is 403 (visible portfolio, wrong domain)' ($r.Status -eq 403) "got $($r.Status)"
Check '  ...and the 403 names the missing domain/action' `
    ($r.Json.domain -eq 'positions' -and $r.Json.action -eq 'view') "got $($r.Raw)"
$r = Invoke-Api -Path "/api/portfolios/$pidA/emir/export" -Cookie $sub
Check 'A/emir/export is 403 (view does not imply export)' ($r.Status -eq 403) "got $($r.Status)"
$ids = @((Invoke-Api -Path '/api/portfolios' -Cookie $sub).Json | ForEach-Object id)
Check 'portfolio list shows exactly A of the two' ($ids -contains $pidA -and $ids -notcontains $pidB) "saw $($ids -join ',')"
$caps = @((Invoke-Api -Path '/api/me' -Cookie $sub).Json.capabilities)
Check '/api/me capability mirrors the grant' `
    ([bool]($caps | Where-Object { $_.domain -eq 'nav' -and $_.action -eq 'view' -and $_.portfolio_id -eq $pidA }))
$r = Invoke-Api -Method POST -Path "/api/admin/users/$subjectId/grants" -Cookie $adm `
    -Body (New-Grant 'nav' 'view' 999999999)
Check 'granting on a nonexistent portfolio is 422' ($r.Status -eq 422) "got $($r.Status)"

# --------------------------------- phase 5: export/import/... imply view

Phase 'Export implies view (server-side, never the reverse)'
$impliedEmail = "e2e-implied-$tag@test.local"
$impliedPw = New-Password
$impliedId = (Invoke-Api -Method POST -Path '/api/admin/users' -Cookie $adm `
        -Body @{ email = $impliedEmail; display_name = "E2E Implied $tag"; password = $impliedPw; is_administrator = $false }).Json.id
$null = Invoke-Api -Method POST -Path "/api/admin/users/$impliedId/grants" -Cookie $adm `
    -Body (New-Grant 'positions' 'export' $pidA)
$imp = (Get-SessionCookie $impliedEmail $impliedPw).Cookie
$r = Invoke-Api -Path "/api/portfolios/$pidA/positions" -Cookie $imp
Check 'positions/export alone allows GET positions (implied view)' ($r.Status -eq 200) "got $($r.Status)"
$r = Invoke-Api -Path "/api/portfolios/$pidA/emir" -Cookie $imp
Check 'EMIR page loads under positions grant (200)' ($r.Status -eq 200) "got $($r.Status)"
if ($r.Json.empty -eq $true) {
    # With no imported snapshots the handler answers {"empty": true} before
    # the denied-Reference marker is built. The populated-portfolio contract
    # (clearing_obligation "unavailable", export 403) is pinned in-process by
    # cargo test -p server (api_emir.rs).
    Skip 'clearing-obligation "unavailable" marker' 'portfolio has no imported snapshots; covered by cargo test api_emir'
} else {
    Check '  ...but its clearing obligation is marked unavailable (reference denied)' `
        ($r.Json.clearing_obligation.status -eq 'unavailable') "got $($r.Raw.Substring(0, [Math]::Min(200, $r.Raw.Length)))"
}
$r = Invoke-Api -Path "/api/portfolios/$pidA/emir/export" -Cookie $imp
Check 'EMIR evidence export never emits a degraded document (403 reference denied, or 422 empty)' `
    ($r.Status -in 403, 422) "got $($r.Status)"

# ------------------------------------------ phase 6: all-portfolios scope

Phase 'All-portfolios (wildcard) scope'
$null = Invoke-Api -Method POST -Path "/api/admin/users/$subjectId/grants" -Cookie $adm `
    -Body (New-Grant 'positions' 'view' $null)
$ids = @((Invoke-Api -Path '/api/portfolios' -Cookie $sub).Json | ForEach-Object id)
Check 'wildcard positions/view makes both A and B visible' ($ids -contains $pidA -and $ids -contains $pidB) "saw $($ids -join ',')"
$r = Invoke-Api -Path "/api/portfolios/$pidB/positions" -Cookie $sub
Check 'B/positions is 200 under the wildcard' ($r.Status -eq 200) "got $($r.Status)"
$r = Invoke-Api -Path "/api/portfolios/$pidB/nav" -Cookie $sub
Check 'B/nav stays 403 (nav grant was A-only; B visible via wildcard)' ($r.Status -eq 403) "got $($r.Status)"
$pidC = (Invoke-Api -Method POST -Path '/api/portfolios' -Cookie $adm -Body @{ name = "E2E-C $tag"; kind = 'ucits' }).Json.id
$ids = @((Invoke-Api -Path '/api/portfolios' -Cookie $sub).Json | ForEach-Object id)
Check 'a portfolio created later is covered by the wildcard automatically' ($ids -contains $pidC) "saw $($ids -join ',')"

# ------------------------------------ phase 7: configure and its limits

Phase 'Configure semantics'
$null = Invoke-Api -Method POST -Path "/api/admin/users/$subjectId/grants" -Cookie $adm `
    -Body (New-Grant 'reference' 'configure' $pidA)
$r = Invoke-Api -Method PUT -Path "/api/portfolios/$pidA" -Cookie $sub -Body @{ name = "E2E-A $tag"; archived = $false }
Check 'per-portfolio reference/configure allows renaming that portfolio' ($r.Status -eq 200) "got $($r.Status)"
$r = Invoke-Api -Method PUT -Path "/api/portfolios/$pidB" -Cookie $sub -Body @{ name = "E2E-B $tag"; archived = $false }
Check 'renaming B is 403 (configure was A-only)' ($r.Status -eq 403) "got $($r.Status)"
$r = Invoke-Api -Method POST -Path '/api/portfolios' -Cookie $sub -Body @{ name = "never2-$tag"; kind = 'ucits' }
Check 'CREATING a portfolio still 403: needs the all-portfolios configure grant' ($r.Status -eq 403) "got $($r.Status)"

# The P10 split: reference/configure is fleet-level bookkeeping (does this
# portfolio exist, what is it called) and must NOT carry the authority to move
# the fund's own risk parameters. That is settings/configure, granted separately.
$r = Invoke-Api -Method GET -Path "/api/portfolios/$pidA/settings" -Cookie $sub
Check 'reference/configure alone cannot even read the fund settings (403)' ($r.Status -eq 403) "got $($r.Status)"
$null = Invoke-Api -Method POST -Path "/api/admin/users/$subjectId/grants" -Cookie $adm `
    -Body (New-Grant 'settings' 'configure' $pidA)
$r = Invoke-Api -Method GET -Path "/api/portfolios/$pidA/settings" -Cookie $sub
Check 'settings/configure grants the fund settings (configure implies view)' ($r.Status -eq 200) "got $($r.Status)"
if ($r.Status -eq 200) {
    $newSettings = $r.Json
    $newSettings.var_limit = [math]::Round($r.Json.var_limit / 2, 6)
    $w = Invoke-Api -Method PUT -Path "/api/portfolios/$pidA/settings" -Cookie $sub -Body $newSettings
    Check '  ...and allows changing the VaR limit' ($w.Status -eq 200) "got $($w.Status)"
}
$r = Invoke-Api -Method GET -Path "/api/portfolios/$pidB/settings" -Cookie $sub
Check "B's settings stay 403 (settings/configure was A-only)" ($r.Status -eq 403) "got $($r.Status)"
$null = Invoke-Api -Method DELETE -Path "/api/admin/users/$subjectId/grants" -Cookie $adm `
    -Body (New-Grant 'settings' 'configure' $pidA)

# ------------------------------------------------------- phase 8: roles

Phase 'Roles: exact bundles, additivity, validation'
$roleEmail = "e2e-roles-$tag@test.local"
$roleId = (Invoke-Api -Method POST -Path '/api/admin/users' -Cookie $adm `
        -Body @{ email = $roleEmail; display_name = "E2E Roles $tag"; password = (New-Password); is_administrator = $false }).Json.id
$r = Invoke-Api -Method POST -Path "/api/admin/users/$roleId/roles" -Cookie $adm -Body @{ role = 'auditor'; scope = $pidA }
Check 'auditor role applies (204)' ($r.Status -eq 204) "got $($r.Status)"
$grants = @((Invoke-Api -Path "/api/admin/users/$roleId/grants" -Cookie $adm).Json)
# Auditor is "view on every domain", so its size is the domain count itself
# rather than a number to be edited whenever a domain is added or split.
$domainCount = $grants.Count
Check 'auditor = one view grant per domain, all scoped to A' `
    (@($grants | Where-Object { $_.action -ne 'view' -or $_.portfolio -ne $pidA }).Count -eq 0 -and $domainCount -ge 7) `
    "got $($grants | ConvertTo-Json -Compress)"
$null = Invoke-Api -Method POST -Path "/api/admin/users/$roleId/roles" -Cookie $adm -Body @{ role = 'operations'; scope = $pidA }
$grants = @((Invoke-Api -Path "/api/admin/users/$roleId/grants" -Cookie $adm).Json)
$imports = @($grants | Where-Object action -eq 'import')
Check 'operations on top is ADDITIVE: every auditor view remains, 4 imports appear' `
    ($grants.Count -eq ($domainCount + 4) -and $imports.Count -eq 4 `
        -and @($grants | Where-Object action -eq 'view').Count -eq $domainCount) `
    "got $($grants.Count) rows, $($imports.Count) imports, expected $($domainCount + 4) rows"
$r = Invoke-Api -Method POST -Path "/api/admin/users/$roleId/roles" -Cookie $adm -Body @{ role = 'not_a_role'; scope = $pidA }
Check 'an unknown role name is 422' ($r.Status -eq 422) "got $($r.Status)"
$r = Invoke-Api -Method POST -Path "/api/admin/users/$roleId/roles" -Cookie $adm -Body @{ role = 'auditor'; scope = 999999999 }
Check 'a role scoped to a nonexistent portfolio is 422' ($r.Status -eq 422) "got $($r.Status)"
$r = Invoke-Api -Method POST -Path "/api/admin/users/999999999/roles" -Cookie $adm -Body @{ role = 'auditor'; scope = $null }
Check 'a role for a nonexistent user is 404' ($r.Status -eq 404) "got $($r.Status)"

# --------------------------------------------- phase 9: live revocation

Phase 'Revocation bites mid-session, without re-login'
$r = Invoke-Api -Method DELETE -Path "/api/admin/users/$subjectId/grants" -Cookie $adm `
    -Body (New-Grant 'nav' 'view' $pidA)
Check 'nav/view on A removed' ($r.Status -eq 204) "got $($r.Status)"
$r = Invoke-Api -Path "/api/portfolios/$pidA/nav" -Cookie $sub
Check 'A/nav is immediately 403 on the live session' ($r.Status -eq 403) "got $($r.Status)"
$null = Invoke-Api -Method DELETE -Path "/api/admin/users/$subjectId/grants" -Cookie $adm -Body (New-Grant 'positions' 'view' $null)
$null = Invoke-Api -Method DELETE -Path "/api/admin/users/$subjectId/grants" -Cookie $adm -Body (New-Grant 'reference' 'configure' $pidA)
$r = Invoke-Api -Path "/api/portfolios/$pidA/nav" -Cookie $sub
Check 'with every grant gone, A reads as nonexistent again (404)' ($r.Status -eq 404) "got $($r.Status)"
$ids = @((Invoke-Api -Path '/api/portfolios' -Cookie $sub).Json | ForEach-Object id)
Check 'portfolio list is back to empty of A/B/C' `
    ($ids -notcontains $pidA -and $ids -notcontains $pidB -and $ids -notcontains $pidC) "saw $($ids -join ',')"

# ----------------------------- phase 10: disable / reset kill sessions

Phase 'Disable and password reset revoke live sessions'
$r = Invoke-Api -Method PUT -Path "/api/admin/users/$impliedId/disabled" -Cookie $adm -Body @{ disabled = $true }
Check 'disable succeeds (204)' ($r.Status -eq 204) "got $($r.Status)"
$r = Invoke-Api -Path '/api/me' -Cookie $imp
Check "the disabled user's live cookie is dead at once (401)" ($r.Status -eq 401) "got $($r.Status)"
$r = Get-SessionCookie $impliedEmail $impliedPw
Check 'a disabled user cannot sign in (401)' ($r.Status -eq 401) "got $($r.Status)"
$null = Invoke-Api -Method PUT -Path "/api/admin/users/$impliedId/disabled" -Cookie $adm -Body @{ disabled = $false }
$r = Invoke-Api -Path '/api/me' -Cookie $imp
Check 're-enabling does NOT resurrect the old session (still 401)' ($r.Status -eq 401) "got $($r.Status)"
$r = Get-SessionCookie $impliedEmail $impliedPw
Check 're-enabled user signs in again with the same password' ($r.Status -eq 200 -and $r.Cookie) "status $($r.Status)"
$imp = $r.Cookie

$impliedPw2 = New-Password
$r = Invoke-Api -Method PUT -Path "/api/admin/users/$impliedId/password" -Cookie $adm -Body @{ password = $impliedPw2 }
Check 'password reset succeeds (204)' ($r.Status -eq 204) "got $($r.Status)"
$r = Invoke-Api -Path '/api/me' -Cookie $imp
Check 'reset kills the live session (401)' ($r.Status -eq 401) "got $($r.Status)"
$r = Get-SessionCookie $impliedEmail $impliedPw
Check 'the old password no longer signs in' ($r.Status -eq 401) "got $($r.Status)"
$r = Get-SessionCookie $impliedEmail $impliedPw2
Check 'the new password signs in' ($r.Status -eq 200 -and $r.Cookie) "status $($r.Status)"
$imp = $r.Cookie

$r = Invoke-Api -Method POST -Path '/api/logout' -Cookie $imp
Check 'logout is 204' ($r.Status -eq 204) "got $($r.Status)"
$r = Invoke-Api -Path '/api/me' -Cookie $imp
Check 'the cookie is dead after logout (401)' ($r.Status -eq 401) "got $($r.Status)"

# ---------------------------------------------------- phase 11: lockout

Phase 'Lockout after five failures'
$lockEmail = "e2e-lockout-$tag@test.local"
if ($SkipLockout) {
    Skip 'lockout test' '-SkipLockout was passed'
} else {
    $lockPw = New-Password
    $null = Invoke-Api -Method POST -Path '/api/admin/users' -Cookie $adm `
        -Body @{ email = $lockEmail; display_name = "E2E Lockout $tag"; password = $lockPw; is_administrator = $false }
    $statuses = for ($i = 1; $i -le 5; $i++) { (Get-SessionCookie $lockEmail 'wrong-password').Status }
    Check 'five wrong passwords are each refused (401/429)' `
        (@($statuses | Where-Object { $_ -notin 401, 429 }).Count -eq 0) "got $($statuses -join ',')"
    $r = Invoke-WebRequest -Method POST -Uri "$BaseUrl/api/login" -SkipHttpErrorCheck `
        -ContentType 'application/json' -Body (@{ email = $lockEmail; password = $lockPw } | ConvertTo-Json)
    Check 'the CORRECT password is now refused too (429 locked out)' ([int]$r.StatusCode -eq 429) "got $($r.StatusCode)"
    $retryAfter = [int]"$($r.Headers['Retry-After'])"
    Check '  ...with a Retry-After of at most 15 minutes' ($retryAfter -ge 1 -and $retryAfter -le 900) "got $retryAfter"
}

# --------------------------------------- phase 12: last-administrator guard

Phase 'Last-administrator guard and enrolment hygiene'
$admin2Email = "e2e-admin2-$tag@test.local"
$admin2Pw = New-Password
$r = Invoke-Api -Method POST -Path '/api/admin/users' -Cookie $adm `
    -Body @{ email = $admin2Email; display_name = "E2E Admin2 $tag"; password = $admin2Pw; is_administrator = $true }
Check 'a second administrator can be created' ($r.Status -eq 200 -and $r.Json.is_administrator) "got $($r.Status)"
$admin2Id = $r.Json.id
$r = Get-SessionCookie $admin2Email $admin2Pw
Check 'the second administrator can sign in' ($r.Status -eq 200 -and $r.Cookie) "status $($r.Status)"
$r = Invoke-Api -Method PUT -Path "/api/admin/users/$admin2Id/disabled" -Cookie $adm -Body @{ disabled = $true }
Check 'disabling an administrator is allowed while another usable one remains' ($r.Status -eq 204) "got $($r.Status)"

$users = @((Invoke-Api -Path '/api/admin/users' -Cookie $adm).Json)
$otherEnabledAdmins = @($users | Where-Object { $_.is_administrator -and -not $_.disabled -and $_.id -ne $adminId })
if ($otherEnabledAdmins.Count -eq 0) {
    $r = Invoke-Api -Method PUT -Path "/api/admin/users/$adminId/disabled" -Cookie $adm -Body @{ disabled = $true }
    Check 'disabling the LAST usable administrator is refused (422)' ($r.Status -eq 422) "got $($r.Status)"
} else {
    Skip 'last-administrator refusal' "other enabled administrators exist ($($otherEnabledAdmins.email -join ', ')) — refusing to risk a real self-disable"
}

# A live session token must never be redeemable as an enrolment token: only
# never-enrolled accounts (sentinel hash) qualify.
$s2 = Get-SessionCookie $subjectEmail $subjectPw
$rawToken = ($s2.Cookie -split '=', 2)[1]
$r = Invoke-Api -Method POST -Path '/api/enrol' -Body @{ token = $rawToken; password = 'hijacked' }
Check "an ordinary session token cannot be replayed through /api/enrol (401)" ($r.Status -eq 401) "got $($r.Status)"
$r = Invoke-Api -Path '/api/me' -Cookie $s2.Cookie
Check '  ...and the probed session itself still works (nothing was consumed)' ($r.Status -eq 200) "got $($r.Status)"

# -------------------------------------------------- phase 13: audit trail

Phase 'Audit trail records this run'
$audit = (Invoke-Api -Path '/api/admin/audit?limit=1000' -Cookie $adm).Raw
$expected = [ordered]@{
    'user_created for the subject'    = $subjectEmail
    'grant_added events'              = 'grant_added'
    'grant_removed events'            = 'grant_removed'
    'role_assigned events'            = 'role_assigned'
    'password_reset event'            = 'password_reset'
    'user_disabled event'             = 'user_disabled'
    'successful login events'         = '"login"'
    'portfolio_create configure event' = 'portfolio_create'
}
if (-not $SkipLockout) {
    $expected['failed-login events'] = 'login_failed'
    $expected['lockout event'] = 'login_locked'
    $expected['locked email recorded as actor detail'] = $lockEmail
}
if ($EnrolToken) { $expected['first-administrator enrolment event'] = 'enrolled' }
foreach ($k in $expected.Keys) {
    Check "audit log contains $k" ($audit -like "*$($expected[$k])*")
}

# ------------------------------------------- fixtures / cleanup / summary

if ($KeepUiFixtures) {
    Phase 'UI fixtures (kept for the manual checklist)'
    # The manual pass starts from: subject holding nav/view + positions/view
    # on A only; A and B active; B is a mandate (for the "(mandat)" suffix).
    $null = Invoke-Api -Method POST -Path "/api/admin/users/$subjectId/grants" -Cookie $adm -Body (New-Grant 'nav' 'view' $pidA)
    $null = Invoke-Api -Method POST -Path "/api/admin/users/$subjectId/grants" -Cookie $adm -Body (New-Grant 'positions' 'view' $pidA)
    $null = Invoke-Api -Method PUT -Path "/api/portfolios/$pidC" -Cookie $adm -Body @{ name = "E2E-C $tag"; archived = $true }
    Write-Host ''
    Write-Host '  Open docs/testing/access-rights-manual-checklist.md and use:' -ForegroundColor Cyan
    Write-Host "    administrator : $AdminEmail / $AdminPassword"
    Write-Host "    subject user  : $subjectEmail / $subjectPw   (nav+positions view on A only)"
    Write-Host "    portfolio A   : E2E-A $tag  (id $pidA, ucits)"
    Write-Host "    portfolio B   : E2E-B $tag  (id $pidB, mandate)"
    if (-not $SkipLockout) { Write-Host "    locked user   : $lockEmail  (locked ~15 min; visible in the audit log)" }
} else {
    Phase 'Cleanup (accounts disabled, portfolios archived)'
    foreach ($id in @($subjectId, $impliedId, $roleId) | Where-Object { $_ }) {
        $null = Invoke-Api -Method PUT -Path "/api/admin/users/$id/disabled" -Cookie $adm -Body @{ disabled = $true }
    }
    foreach ($p in @(@($pidA, "E2E-A $tag"), @($pidB, "E2E-B $tag"), @($pidC, "E2E-C $tag"))) {
        if ($p[0]) {
            $null = Invoke-Api -Method PUT -Path "/api/portfolios/$($p[0])" -Cookie $adm -Body @{ name = $p[1]; archived = $true }
        }
    }
    if (-not $hadGlobalRefConfigure) {
        $null = Invoke-Api -Method DELETE -Path "/api/admin/users/$adminId/grants" -Cookie $adm `
            -Body (New-Grant 'reference' 'configure' $null)
    }
    Write-Host '  done (the lockout user, if any, stays locked until its window expires)'
}

Write-Host ''
Write-Host ("=" * 60)
Write-Host "PASS $script:Pass   FAIL $script:Fail   SKIP $script:Skipped"
if ($script:Fail -gt 0) {
    Write-Host 'failures:' -ForegroundColor Red
    $script:Failures | ForEach-Object { Write-Host "  - $_" -ForegroundColor Red }
    exit 1
}
exit 0
