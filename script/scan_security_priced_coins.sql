-- Reports coin prices that were taken from a same-ticker security. Writes nothing.
--
--   sqlite3 -readonly -header -column storage/production.sqlite3 < script/scan_security_priced_coins.sql
--
-- Until the price service kept the two apart, a broker row whose name is also a security's (Alpaca
-- lists a BTC security beside BTC/USD) could be priced as that security, and its closes were stored
-- under the bare symbol, which every coin of that symbol reads. Those rows never change on their own:
-- prices are only ever inserted.
--
-- A symbol is in scope when the catalogue has it both as a security and as a coin, and this
-- instance's rows name it (as base, quote or fee) at a stock venue. A bare-symbol price row of such a
-- symbol is a suspect when it breaks its own series' scale: more than 100 times below or above the
-- median price of that symbol and currency within 30 days either side. A security's close
-- and a coin's price differ by orders of magnitude (a $36 fund share beside an $83,000 bitcoin); a
-- coin's own daily moves do not. The `stock:SYM` rows are no reference: before the backfill told the
-- two apart, they could hold the coin's prices too.
-- The second statement lists the users whose stored history covers a suspect day: every one of
-- them, a user with no row at a broker included, read those prices.

WITH securities AS (
  SELECT DISTINCT symbol FROM assets WHERE category IN ('Stock', 'Common Stock', 'ETF', 'Fund')
), shared AS (
  SELECT DISTINCT a.symbol FROM assets a JOIN securities s ON s.symbol = a.symbol WHERE a.category = 'Cryptocurrency'
), touched AS (
  SELECT DISTINCT s.symbol FROM shared s
  JOIN account_transactions t ON s.symbol IN (t.base_currency, t.quote_currency, t.fee_currency)
  JOIN exchanges e ON e.id = t.exchange_id
  WHERE e.type IN ('Exchanges::Alpaca', 'Exchanges::Ibkr')
), neighbours AS (
  SELECT p.id, CAST(n.price AS REAL) AS value,
         row_number() OVER (PARTITION BY p.id ORDER BY CAST(n.price AS REAL)) AS position,
         count(*) OVER (PARTITION BY p.id) AS size
  FROM historical_prices p JOIN touched t ON t.symbol = p.asset
  JOIN historical_prices n ON n.asset = p.asset AND n.currency = p.currency
                          AND n.date BETWEEN date(p.date, '-30 days') AND date(p.date, '+30 days')
), priced AS (
  SELECT p.id, p.asset, p.currency, p.date, p.price, m.value AS nearby_median
  FROM historical_prices p JOIN neighbours m ON m.id = p.id AND m.position = m.size / 2 + 1
), suspects AS (
  SELECT * FROM priced WHERE CAST(price AS REAL) * 100 < nearby_median OR CAST(price AS REAL) > nearby_median * 100
)
SELECT id, asset, currency, date, price, nearby_median FROM suspects ORDER BY asset, currency, date;

WITH securities AS (
  SELECT DISTINCT symbol FROM assets WHERE category IN ('Stock', 'Common Stock', 'ETF', 'Fund')
), shared AS (
  SELECT DISTINCT a.symbol FROM assets a JOIN securities s ON s.symbol = a.symbol WHERE a.category = 'Cryptocurrency'
), touched AS (
  SELECT DISTINCT s.symbol FROM shared s
  JOIN account_transactions t ON s.symbol IN (t.base_currency, t.quote_currency, t.fee_currency)
  JOIN exchanges e ON e.id = t.exchange_id
  WHERE e.type IN ('Exchanges::Alpaca', 'Exchanges::Ibkr')
), neighbours AS (
  SELECT p.id, CAST(n.price AS REAL) AS value,
         row_number() OVER (PARTITION BY p.id ORDER BY CAST(n.price AS REAL)) AS position,
         count(*) OVER (PARTITION BY p.id) AS size
  FROM historical_prices p JOIN touched t ON t.symbol = p.asset
  JOIN historical_prices n ON n.asset = p.asset AND n.currency = p.currency
                          AND n.date BETWEEN date(p.date, '-30 days') AND date(p.date, '+30 days')
), priced AS (
  SELECT p.id, p.asset, p.currency, p.date, p.price, m.value AS nearby_median
  FROM historical_prices p JOIN neighbours m ON m.id = p.id AND m.position = m.size / 2 + 1
), suspects AS (
  SELECT * FROM priced WHERE CAST(price AS REAL) * 100 < nearby_median OR CAST(price AS REAL) > nearby_median * 100
)
SELECT DISTINCT user_id FROM (
  SELECT user_id, date FROM portfolio_snapshots UNION ALL SELECT user_id, date FROM portfolio_venue_snapshots
) s WHERE s.date IN (SELECT date FROM suspects) ORDER BY user_id;
