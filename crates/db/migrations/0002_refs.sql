CREATE TABLE instrument_refs (
  code TEXT PRIMARY KEY,
  issuer_group TEXT,
  liquidity_bucket TEXT CHECK (liquidity_bucket IN ('d1','d2_7','d8_30','d30p')),
  bond_coupon_pct NUMERIC CHECK (bond_coupon_pct >= 0),
  bond_maturity DATE,
  bond_coupon_freq INT CHECK (bond_coupon_freq IN (1,2)),
  updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

INSERT INTO settings (key, value) VALUES
  ('liquidity_defaults', '{"Action":"d1","Fonds":"d2_7","Future":"d1","Obligation":"d8_30","Cash Acc":"d1","Margin Acc":"d1","Dividendes":"d1","Frais provisionnés":"d1","Provisions ordres":"d1"}'),
  ('redemption_shock', '0.30');
