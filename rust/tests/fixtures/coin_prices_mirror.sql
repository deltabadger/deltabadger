-- Test support (delisted coins, plan r10 §1.5): a coin's prices are read from asset_prices by its identity, and 2.x's
-- symbol-keyed rows stay unread (option A). Recorded installs and hand-seeded tests keep their coin prices in
-- historical_prices under a symbol; this copies each row of a symbol exactly one catalogue Cryptocurrency carries into
-- asset_prices under that coin, now and on every later insert: the provider serving the same prices by identity. The store
-- has asset_prices (its migration) before this runs.
INSERT OR IGNORE INTO asset_prices (external_id, currency, date, price)
  SELECT (SELECT a.external_id FROM assets a WHERE a.symbol = h.asset AND a.category = 'Cryptocurrency'), h.currency, h.date, CAST(h.price AS TEXT)
  FROM historical_prices h WHERE (SELECT count(*) FROM assets a WHERE a.symbol = h.asset AND a.category = 'Cryptocurrency') = 1 ORDER BY h.id;
CREATE TRIGGER IF NOT EXISTS test_mirror_coin_prices AFTER INSERT ON historical_prices
  WHEN (SELECT count(*) FROM assets a WHERE a.symbol = NEW.asset AND a.category = 'Cryptocurrency') = 1
  BEGIN
    INSERT OR IGNORE INTO asset_prices (external_id, currency, date, price)
      SELECT a.external_id, NEW.currency, NEW.date, CAST(NEW.price AS TEXT) FROM assets a WHERE a.symbol = NEW.asset AND a.category = 'Cryptocurrency';
  END;
