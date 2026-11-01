-- PLAN-A12 A12-2c-3, D-TAX-PL-DECLARED-CARRY: column d of PIT-38 part E as the user declares it for a tax year, in PLN
-- (for 2019 the costs incurred before 2019 and never deducted; for later years column f of the previous return). The PL
-- report's chain takes it at that year in place of the f carried from the year before. Written only by a generate
-- request that is queued (web, REST, MCP), so it never changes under a queued or running report.
CREATE TABLE rust_tax_pl_carry (
  user_id INTEGER NOT NULL,
  year INTEGER NOT NULL,
  costs TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  PRIMARY KEY (user_id, year)
);
