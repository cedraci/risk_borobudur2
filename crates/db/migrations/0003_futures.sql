CREATE TABLE futures_contracts (
  contract_root    TEXT PRIMARY KEY,
  label            TEXT NOT NULL,
  category         TEXT NOT NULL CHECK (category IN
                   ('equity','interest_rate','fx','credit','commodity','other')),
  point_value      NUMERIC CHECK (point_value > 0),
  currency         TEXT NOT NULL,
  curve            TEXT,
  price_convention TEXT NOT NULL DEFAULT 'decimal'
                   CHECK (price_convention IN ('decimal','th32')),
  confirmed        BOOLEAN NOT NULL DEFAULT false,
  updated_at       TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE futures_analytics (
  nav_date          DATE NOT NULL,
  ticker            TEXT NOT NULL,
  ctd_isin          TEXT NOT NULL,
  ctd_mod_duration  NUMERIC NOT NULL CHECK (ctd_mod_duration > 0),
  ctd_clean_price   NUMERIC NOT NULL CHECK (ctd_clean_price > 0),
  ctd_accrued       NUMERIC NOT NULL DEFAULT 0 CHECK (ctd_accrued >= 0),
  conversion_factor NUMERIC NOT NULL CHECK (conversion_factor > 0),
  source_file       TEXT NOT NULL,
  uploaded_at       TIMESTAMPTZ NOT NULL DEFAULT now(),
  PRIMARY KEY (nav_date, ticker)
);
