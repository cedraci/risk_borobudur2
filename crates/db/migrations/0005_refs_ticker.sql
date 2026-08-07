-- Bloomberg-resolved ticker, pulled alongside the classification fields.
-- Stored for later use (the NAV Recap itself has no usable ticker column).
ALTER TABLE instrument_refs
  ADD COLUMN ticker TEXT;
