-- PLAN-A12 A12-3, D-TAX-CZ-CNB: the ČNB's daily fixing (ZDP § 38 odst. 1 písm. b) bod 1, odst. 5), CZK per one unit of
-- `currency`, as the year files at www.cnb.cz publish them (value ÷ amount). `cnb_coverage` records the days a fetched year
-- file proves (1 January to its last fixing, and on to the year's end or yesterday when the daily fixing confirms no later
-- one), so a covered day with no fixing reads the last fixing before it, and a day outside coverage has no rate.
CREATE TABLE cnb_rates (
  currency TEXT NOT NULL,
  date TEXT NOT NULL,
  rate TEXT NOT NULL,
  UNIQUE (currency, date)
);
CREATE TABLE cnb_coverage (
  from_date TEXT NOT NULL,
  to_date TEXT NOT NULL
);
CREATE INDEX index_cnb_coverage_on_from_date ON cnb_coverage (from_date);
