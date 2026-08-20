-- Finding P10: `reference` gated two authorities that were never the same
-- one. Curating the shared instrument tables — classifications, issuer
-- groups, bond statics, futures specs, FX — is fleet-wide by nature. Setting
-- a fund's own VaR limit, redemption stress, liquidity parameters, depositary
-- code mapping and monthly EMIR KPIs is not, and an instance-wide
-- `reference/configure` grant should never have carried it.
--
-- `settings` takes the per-portfolio half.

ALTER TABLE grants DROP CONSTRAINT grants_domain_check;
ALTER TABLE grants ADD CONSTRAINT grants_domain_check CHECK (domain IN
  ('positions','nav','transactions','shareholders','market_data','reference','settings'));

-- Nobody loses anything on upgrade. Everyone holding a `reference` grant was,
-- by construction, holding the settings authority too, so each one gets its
-- twin — same action, same scope. Whether to narrow those afterwards is an
-- administrator's decision, made row by row on the grant matrix, not
-- something a migration should make on their behalf.
INSERT INTO grants (user_id, domain, action, portfolio_id, granted_by)
SELECT user_id, 'settings', action, portfolio_id, granted_by
FROM grants
WHERE domain = 'reference'
ON CONFLICT DO NOTHING;
