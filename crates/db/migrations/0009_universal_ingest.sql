-- External identifiers used to auto-route self-identifying uploads
-- (e.g. CACEIS fund code 165878) to a portfolio. One code maps to exactly
-- one portfolio per source; a portfolio may hold several codes.
CREATE TABLE portfolio_codes (
  portfolio_id BIGINT NOT NULL REFERENCES portfolios(id),
  source       TEXT NOT NULL,
  code         TEXT NOT NULL,
  PRIMARY KEY (source, code)
);

-- Dividend rows derived from CACEIS CPON receivable deltas are flagged so
-- the derivation can delete-and-rebuild its own rows without touching
-- explicit (file-sourced) journal entries.
ALTER TABLE dividends ADD COLUMN derived BOOLEAN NOT NULL DEFAULT false;
