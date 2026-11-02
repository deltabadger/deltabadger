-- PLAN-A12 A12-6, D-TAX-CH-ESTV: the ESTV Kursliste's year-end tax value of a crypto asset (ictax), CHF per unit, by symbol
-- as ESTV lists it. `value` NULL: asked and not listed by ESTV that year (the report values it at the market price).
CREATE TABLE estv_crypto_values (
  year INTEGER NOT NULL,
  symbol TEXT NOT NULL,
  value TEXT,
  UNIQUE (year, symbol)
);
