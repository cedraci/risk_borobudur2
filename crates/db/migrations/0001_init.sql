CREATE TABLE imports (
  id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
  filename TEXT NOT NULL,
  sha256 TEXT NOT NULL UNIQUE,
  nav_date DATE NOT NULL,
  imported_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  row_counts JSONB NOT NULL
);

CREATE TABLE nav_history (
  date DATE PRIMARY KEY,
  aum NUMERIC NOT NULL,
  shares NUMERIC NOT NULL,
  nav NUMERIC NOT NULL
);

CREATE TABLE position_snapshots (
  id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
  nav_date DATE NOT NULL,
  import_id BIGINT NOT NULL REFERENCES imports(id) ON DELETE CASCADE,
  asset_type TEXT NOT NULL,
  isin TEXT NOT NULL,
  name TEXT,
  currency TEXT,
  quantity NUMERIC,
  avg_cost NUMERIC,
  price NUMERIC,
  valuation_ccy NUMERIC,
  accrued_interest NUMERIC,
  fx_rate NUMERIC,
  valuation_eur NUMERIC,
  weight NUMERIC,
  ticker TEXT
);
CREATE INDEX idx_positions_nav_date ON position_snapshots(nav_date);

CREATE TABLE dividends (
  id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
  provision_date DATE NOT NULL,
  payment_date DATE,
  issuer TEXT NOT NULL,
  amount NUMERIC NOT NULL,
  currency TEXT NOT NULL
);

CREATE TABLE operations (
  id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
  trade_date DATE NOT NULL,
  side TEXT NOT NULL,
  ticker TEXT,
  isin TEXT,
  name TEXT,
  currency TEXT,
  quantity NUMERIC,
  price NUMERIC,
  gross_amount NUMERIC,
  fees NUMERIC,
  net_price NUMERIC,
  net_amount NUMERIC
);

CREATE TABLE settings (
  key TEXT PRIMARY KEY,
  value JSONB NOT NULL
);

INSERT INTO settings (key, value) VALUES
  ('risk_free_rate', '0.02'),
  ('var_confidence', '0.99'),
  ('var_horizon_days', '20'),
  ('var_window_days', '252'),
  ('var_limit', '0.20'),
  ('short_dd_max_days', '50');
