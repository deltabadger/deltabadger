-- PLAN-A12 A12-2b, D-TAX-PL-NBP: the NBP's table A mid rates (art. 11a PIT act: the last table before the day of the
-- revenue or cost), PLN per unit of `currency`, as api.nbp.pl publishes them. `nbp_coverage` records each range fetched in
-- full, so a covered day with no table is a day NBP published none (a holiday), and a day outside coverage is unknown: the
-- report reads "the last table before" only across covered days, never falling back to an older stored table.
CREATE TABLE nbp_rates (
  currency TEXT NOT NULL,
  date TEXT NOT NULL,
  rate TEXT NOT NULL,
  UNIQUE (currency, date)
);
CREATE TABLE nbp_coverage (
  currency TEXT NOT NULL,
  from_date TEXT NOT NULL,
  to_date TEXT NOT NULL
);
CREATE INDEX index_nbp_coverage_on_currency ON nbp_coverage (currency, from_date);
