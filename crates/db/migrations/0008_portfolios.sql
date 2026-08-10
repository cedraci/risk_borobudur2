-- Phase 1 of the multi-portfolio re-architecture: a portfolio dimension.
-- Existing data belongs to the Borobudur UCITS fund, seeded here as id 1
-- (fresh table => identity starts at 1, on the live DB and in tests alike).
-- Instrument/market reference data (instrument_refs, futures_contracts,
-- fx_history) stays shared: facts about instruments, not portfolios.

CREATE TABLE portfolios (
  id         BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
  name       TEXT NOT NULL UNIQUE,
  kind       TEXT NOT NULL CHECK (kind IN ('ucits','mandate')),
  archived   BOOLEAN NOT NULL DEFAULT false,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

INSERT INTO portfolios (name, kind) VALUES ('Borobudur', 'ucits');

-- Add portfolio_id everywhere time-series, backfilling existing rows to
-- portfolio 1, then drop the default: new writes must name their portfolio.

ALTER TABLE imports            ADD COLUMN portfolio_id BIGINT NOT NULL DEFAULT 1 REFERENCES portfolios(id);
ALTER TABLE nav_history        ADD COLUMN portfolio_id BIGINT NOT NULL DEFAULT 1 REFERENCES portfolios(id);
ALTER TABLE position_snapshots ADD COLUMN portfolio_id BIGINT NOT NULL DEFAULT 1 REFERENCES portfolios(id);
ALTER TABLE dividends          ADD COLUMN portfolio_id BIGINT NOT NULL DEFAULT 1 REFERENCES portfolios(id);
ALTER TABLE operations         ADD COLUMN portfolio_id BIGINT NOT NULL DEFAULT 1 REFERENCES portfolios(id);
ALTER TABLE futures_analytics  ADD COLUMN portfolio_id BIGINT NOT NULL DEFAULT 1 REFERENCES portfolios(id);
ALTER TABLE emir_kpis          ADD COLUMN portfolio_id BIGINT NOT NULL DEFAULT 1 REFERENCES portfolios(id);
ALTER TABLE settings           ADD COLUMN portfolio_id BIGINT NOT NULL DEFAULT 1 REFERENCES portfolios(id);

ALTER TABLE imports            ALTER COLUMN portfolio_id DROP DEFAULT;
ALTER TABLE nav_history        ALTER COLUMN portfolio_id DROP DEFAULT;
ALTER TABLE position_snapshots ALTER COLUMN portfolio_id DROP DEFAULT;
ALTER TABLE dividends          ALTER COLUMN portfolio_id DROP DEFAULT;
ALTER TABLE operations         ALTER COLUMN portfolio_id DROP DEFAULT;
ALTER TABLE futures_analytics  ALTER COLUMN portfolio_id DROP DEFAULT;
ALTER TABLE emir_kpis          ALTER COLUMN portfolio_id DROP DEFAULT;
ALTER TABLE settings           ALTER COLUMN portfolio_id DROP DEFAULT;

-- Re-key the tables whose identity was previously "the fund's".
ALTER TABLE nav_history       DROP CONSTRAINT nav_history_pkey;
ALTER TABLE nav_history       ADD PRIMARY KEY (portfolio_id, date);
ALTER TABLE imports           DROP CONSTRAINT imports_sha256_key;
ALTER TABLE imports           ADD CONSTRAINT imports_portfolio_sha256_key UNIQUE (portfolio_id, sha256);
ALTER TABLE futures_analytics DROP CONSTRAINT futures_analytics_pkey;
ALTER TABLE futures_analytics ADD PRIMARY KEY (portfolio_id, nav_date, ticker);
ALTER TABLE emir_kpis         DROP CONSTRAINT emir_kpis_pkey;
ALTER TABLE emir_kpis         ADD PRIMARY KEY (portfolio_id, month);
ALTER TABLE settings          DROP CONSTRAINT settings_pkey;
ALTER TABLE settings          ADD PRIMARY KEY (portfolio_id, key);

CREATE INDEX position_snapshots_portfolio_date_idx ON position_snapshots (portfolio_id, nav_date);
