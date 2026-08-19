-- Re-assigning the same role at the same scope must be idempotent, not a
-- constraint violation: `Admin::role_assign` now upserts via
-- `ON CONFLICT (user_id, role, portfolio_id) DO NOTHING`, which needs a real
-- unique target to conflict on. NULLS NOT DISTINCT so two instance-wide rows
-- (portfolio_id NULL) collide, mirroring idx_grants_unique.
CREATE UNIQUE INDEX idx_user_roles_unique
  ON user_roles(user_id, role, portfolio_id) NULLS NOT DISTINCT;
