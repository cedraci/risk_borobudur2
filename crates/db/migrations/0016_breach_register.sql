-- The limit breach register. See
-- docs/superpowers/specs/2026-08-20-limit-breach-register-design.md.
--
-- Runs and results are immutable: nothing in the application updates them.
-- A limit lowered tomorrow cannot rewrite what a run said yesterday.

CREATE TABLE limit_check_runs (
  id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
  portfolio_id BIGINT NOT NULL REFERENCES portfolios(id) ON DELETE CASCADE,
  nav_date DATE NOT NULL,
  run_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  triggered_by TEXT NOT NULL CHECK (triggered_by IN ('import','manual')),
  import_id BIGINT REFERENCES imports(id) ON DELETE SET NULL,
  actor_user_id BIGINT REFERENCES users(id) ON DELETE SET NULL,
  -- false when an input was genuinely absent (no shareholder register, no CTD
  -- analytics for the date). Never false because of a permission: a run
  -- computes under the system context.
  inputs_complete BOOLEAN NOT NULL DEFAULT true,
  input_notes JSONB NOT NULL DEFAULT '{}'::jsonb
);
CREATE UNIQUE INDEX idx_runs_unique ON limit_check_runs(portfolio_id, nav_date, run_at);
CREATE INDEX idx_runs_portfolio_date ON limit_check_runs(portfolio_id, nav_date DESC);

CREATE TABLE limit_check_results (
  id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
  run_id BIGINT NOT NULL REFERENCES limit_check_runs(id) ON DELETE CASCADE,
  check_key TEXT NOT NULL,
  scope_label TEXT NOT NULL,
  -- Both nullable: a check whose verdict comes from a waterfall rather than a
  -- threshold has no honest scalar pair, and renders from status + detail.
  limit_value DOUBLE PRECISION,
  observed_value DOUBLE PRECISION,
  status TEXT NOT NULL CHECK (status IN ('ok','watch','breach')),
  detail JSONB NOT NULL DEFAULT '{}'::jsonb
);
CREATE UNIQUE INDEX idx_results_unique ON limit_check_results(run_id, check_key);

CREATE TABLE limit_breaches (
  id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
  portfolio_id BIGINT NOT NULL REFERENCES portfolios(id) ON DELETE CASCADE,
  check_key TEXT NOT NULL,
  subject TEXT NOT NULL,
  opened_run_id BIGINT NOT NULL REFERENCES limit_check_runs(id) ON DELETE CASCADE,
  opened_nav_date DATE NOT NULL,
  opened_value DOUBLE PRECISION,
  peak_value DOUBLE PRECISION,
  peak_nav_date DATE,
  closed_run_id BIGINT REFERENCES limit_check_runs(id) ON DELETE SET NULL,
  closed_nav_date DATE,
  state TEXT NOT NULL DEFAULT 'open' CHECK (state IN ('open','acknowledged','resolved')),
  classification TEXT NOT NULL DEFAULT 'unclassified'
    CHECK (classification IN ('unclassified','active','passive')),
  proposed_classification TEXT CHECK (proposed_classification IN ('active','passive')),
  proposal_reason TEXT,
  acknowledged_by BIGINT REFERENCES users(id) ON DELETE SET NULL,
  acknowledged_at TIMESTAMPTZ,
  acknowledgement_note TEXT,
  deadline_date DATE,
  resolved_by BIGINT REFERENCES users(id) ON DELETE SET NULL,
  resolved_at TIMESTAMPTZ,
  resolution_note TEXT
);
-- At most one episode per subject that is still in breach on the data. An
-- episode that has cleared but awaits sign-off deliberately does NOT block a
-- new one: a fresh breach next week is a second thing to explain.
CREATE UNIQUE INDEX idx_breaches_live
  ON limit_breaches(portfolio_id, check_key, subject)
  WHERE closed_nav_date IS NULL AND state <> 'resolved';
CREATE INDEX idx_breaches_portfolio ON limit_breaches(portfolio_id, state);

CREATE TABLE limit_breach_events (
  id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
  breach_id BIGINT NOT NULL REFERENCES limit_breaches(id) ON DELETE CASCADE,
  at TIMESTAMPTZ NOT NULL DEFAULT now(),
  actor_user_id BIGINT REFERENCES users(id) ON DELETE SET NULL,
  actor_label TEXT NOT NULL,
  event TEXT NOT NULL CHECK (event IN
    ('opened','classified','acknowledged','note','cleared','resolved','reopened')),
  detail JSONB NOT NULL DEFAULT '{}'::jsonb
);
CREATE INDEX idx_breach_events_breach ON limit_breach_events(breach_id, at);
