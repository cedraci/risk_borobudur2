ALTER TABLE instrument_refs
  ADD COLUMN country_of_risk TEXT,
  ADD COLUMN region          TEXT,
  ADD COLUMN gics_sector     TEXT,
  ADD COLUMN gics_industry   TEXT,
  ADD COLUMN classified_at   TIMESTAMPTZ;

CREATE TABLE fx_history (
  date        DATE NOT NULL,
  currency    TEXT NOT NULL,
  rate_to_eur NUMERIC NOT NULL CHECK (rate_to_eur > 0),
  PRIMARY KEY (date, currency)
);

CREATE INDEX idx_fx_history_currency ON fx_history(currency, date);
