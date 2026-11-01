-- PLAN-A12 A12-1: a tax report asked for (D-TAX-QUEUE). The row is Rails' Solid Queue job and its concurrency slot
-- `tax_report_<user>_<scope>`: at most one `queued` or `running` row per (user, scope), so an INSERT that meets the
-- partial unique index is Rails' discard on conflict. A run ends `ready`, `refused` or `failed`, and that row stays as the
-- request's outcome until the next request for its (user, scope) is accepted. `request` names the request's panel
-- (`tax-report-<request>`) and its staging files (`<path>.stale.<request>`, `<path>.tmp.<request>`). `locale` is the
-- request's, as ActiveJob restores I18n.locale for the run's broadcasts.
CREATE TABLE rust_tax_reports (
  id INTEGER PRIMARY KEY,
  request TEXT NOT NULL UNIQUE,
  user_id INTEGER NOT NULL,
  scope TEXT NOT NULL,
  country TEXT NOT NULL,
  year INTEGER NOT NULL,
  stablecoin_as_fiat INTEGER NOT NULL,
  locale TEXT NOT NULL,
  state TEXT NOT NULL,
  created_at TEXT NOT NULL,
  started_at TEXT,
  finished_at TEXT
);
CREATE UNIQUE INDEX index_rust_tax_reports_slot ON rust_tax_reports (user_id, scope) WHERE state IN ('queued', 'running');
CREATE INDEX index_rust_tax_reports_on_state ON rust_tax_reports (state, id);
