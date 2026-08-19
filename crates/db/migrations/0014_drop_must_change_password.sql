-- The column was created by 0012 but never set true and never checked at
-- login; password changes are forced operationally (admin reset kills the
-- user's sessions), so the flag is dead weight.
ALTER TABLE users DROP COLUMN must_change_password;
