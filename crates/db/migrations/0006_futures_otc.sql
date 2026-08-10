-- EMIR clearing thresholds count OTC positions only. A contract executed on
-- an EU regulated market or an equivalent third-country market is not OTC;
-- one on a non-equivalent venue is, even if exchange-listed. Default false:
-- every contract currently on record is listed on an equivalent venue.
ALTER TABLE futures_contracts
  ADD COLUMN otc BOOLEAN NOT NULL DEFAULT false;
