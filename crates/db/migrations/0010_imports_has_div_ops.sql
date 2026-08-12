-- The replace-if-latest gate for the dividends/operations journal must only
-- ever compare against imports that actually carry (or could carry) that
-- journal. Before this branch every `imports` row came from a NAV Recap
-- (journal-bearing), so DEFAULT true is the correct backfill for existing
-- rows; CACEIS CSV imports (added by this branch) never carry a journal and
-- must set this to false explicitly on insert.
ALTER TABLE imports ADD COLUMN has_div_ops BOOLEAN NOT NULL DEFAULT true;
