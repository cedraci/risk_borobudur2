-- Monthly EMIR KPIs for the risk committee: confirmation follow-up,
-- reconciliation status and dispute count are middle-office facts the tool
-- cannot derive, so they are entered by hand, one row per calendar month.
CREATE TABLE emir_kpis (
  month               DATE PRIMARY KEY
                      CHECK (month = date_trunc('month', month)::date),
  unconfirmed_over_5d INT NOT NULL CHECK (unconfirmed_over_5d >= 0),
  reconciliation      TEXT NOT NULL
                      CHECK (reconciliation IN ('done','not_done','not_applicable')),
  disputes            INT NOT NULL CHECK (disputes >= 0),
  note                TEXT,
  updated_at          TIMESTAMPTZ NOT NULL DEFAULT now()
);
