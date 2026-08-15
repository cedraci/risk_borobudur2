-- Authentication and authorization. New tables only: an existing desktop
-- database upgrades by running this and continues to work as a single-user
-- install, because desktop mode never reads any of it.

CREATE TABLE users (
  id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
  email TEXT NOT NULL UNIQUE,
  display_name TEXT NOT NULL,
  password_hash TEXT NOT NULL,
  is_administrator BOOLEAN NOT NULL DEFAULT false,
  disabled BOOLEAN NOT NULL DEFAULT false,
  must_change_password BOOLEAN NOT NULL DEFAULT false,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- Sessions are server-side so revocation is immediate. Only the hash of the
-- token is stored: a stolen database gives no usable cookie.
CREATE TABLE sessions (
  token_hash TEXT PRIMARY KEY,
  user_id BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  expires_at TIMESTAMPTZ NOT NULL
);
CREATE INDEX idx_sessions_user ON sessions(user_id);
CREATE INDEX idx_sessions_expiry ON sessions(expires_at);

-- One row per (subject, domain, action, portfolio). NULL portfolio_id means
-- every portfolio, and is the only thing that reaches instance-wide resources.
CREATE TABLE grants (
  id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
  user_id BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  domain TEXT NOT NULL CHECK (domain IN
    ('positions','nav','transactions','shareholders','market_data','reference')),
  action TEXT NOT NULL CHECK (action IN ('view','export','import','configure')),
  portfolio_id BIGINT REFERENCES portfolios(id) ON DELETE CASCADE,
  granted_by BIGINT REFERENCES users(id) ON DELETE SET NULL,
  granted_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
-- NULLS NOT DISTINCT so a second wildcard row for the same pair collides.
CREATE UNIQUE INDEX idx_grants_unique
  ON grants(user_id, domain, action, portfolio_id) NULLS NOT DISTINCT;
CREATE INDEX idx_grants_user ON grants(user_id);

-- Which template a user was given, and at what scope. Kept only so the
-- administration screen can offer "re-apply this role"; never read at request
-- time, because roles expand into grant rows at assignment.
CREATE TABLE user_roles (
  id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
  user_id BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  role TEXT NOT NULL CHECK (role IN ('risk_analyst','head_of_risk','operations','auditor')),
  portfolio_id BIGINT REFERENCES portfolios(id) ON DELETE CASCADE,
  assigned_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX idx_user_roles_user ON user_roles(user_id);

-- Append-only. There is deliberately no delete path in the application.
-- actor_label denormalises the display name so history stays readable after a
-- user is deleted; user_id goes NULL rather than taking the row with it.
CREATE TABLE audit_events (
  id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
  at TIMESTAMPTZ NOT NULL DEFAULT now(),
  user_id BIGINT REFERENCES users(id) ON DELETE SET NULL,
  actor_label TEXT NOT NULL,
  action TEXT NOT NULL,
  domain TEXT,
  portfolio_id BIGINT,
  detail JSONB NOT NULL DEFAULT '{}'::jsonb,
  source_addr TEXT
);
CREATE INDEX idx_audit_at ON audit_events(at DESC);
CREATE INDEX idx_audit_user ON audit_events(user_id);

-- Per account, not per IP: a corporate NAT must not lock out a whole floor.
CREATE TABLE login_attempts (
  email TEXT PRIMARY KEY,
  failures INT NOT NULL DEFAULT 0,
  last_failure_at TIMESTAMPTZ,
  locked_until TIMESTAMPTZ
);
