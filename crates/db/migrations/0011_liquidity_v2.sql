ALTER TABLE instrument_refs
    ADD COLUMN adv_30d           NUMERIC,
    ADD COLUMN adv_asof          DATE,
    ADD COLUMN liquidity_days    NUMERIC,
    ADD COLUMN market_place      TEXT,
    ADD COLUMN market_place_name TEXT,
    ADD COLUMN bond_next_coupon  DATE,
    ADD COLUMN bond_nominal      NUMERIC,
    ADD COLUMN adv_eligible      BOOLEAN;

-- Carry every existing override forward at its band's conservative upper edge.
UPDATE instrument_refs SET liquidity_days = CASE liquidity_bucket
    WHEN 'd1' THEN 1 WHEN 'd2_7' THEN 7 WHEN 'd8_30' THEN 30 WHEN 'd30p' THEN 60 END
    WHERE liquidity_bucket IS NOT NULL;

ALTER TABLE instrument_refs DROP COLUMN liquidity_bucket;

ALTER TABLE instrument_refs ADD CONSTRAINT instrument_refs_liquidity_days_nonneg
    CHECK (liquidity_days IS NULL OR liquidity_days >= 0);
ALTER TABLE instrument_refs ADD CONSTRAINT instrument_refs_adv_nonneg
    CHECK (adv_30d IS NULL OR adv_30d >= 0);
ALTER TABLE instrument_refs ADD CONSTRAINT instrument_refs_bond_nominal_pos
    CHECK (bond_nominal IS NULL OR bond_nominal > 0);

CREATE TABLE shareholders (
    id           BIGSERIAL PRIMARY KEY,
    portfolio_id BIGINT  NOT NULL REFERENCES portfolios(id),
    label        TEXT    NOT NULL,
    pct_of_nav   NUMERIC NOT NULL CHECK (pct_of_nav > 0 AND pct_of_nav <= 100),
    as_of        DATE    NOT NULL
);
CREATE INDEX shareholders_portfolio_idx ON shareholders (portfolio_id);

CREATE TABLE share_class_flows (
    portfolio_id        BIGINT  NOT NULL REFERENCES portfolios(id),
    flow_date           DATE    NOT NULL,
    share_class         TEXT    NOT NULL,
    outstanding_shares  NUMERIC,
    nav_per_share       NUMERIC,
    subscription_amount NUMERIC NOT NULL,
    redemption_amount   NUMERIC NOT NULL,
    PRIMARY KEY (portfolio_id, flow_date, share_class)
);
