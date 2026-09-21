# Gemini lists its perpetuals in the same symbol list as spot and under the same base and quote
# (BTCGUSDPERP is BTC/GUSD, like BTCGUSD). The catalogue sync matches rows on base and quote, so a
# row could end up holding the perpetual's symbol and sizes, and orders for that pair went to the
# perpetual. The sync now reads spot only; this puts the rows it already wrote back on spot.
#
# Each Gemini row whose symbol ends in "perp" either takes its spot twin from the snapshot below, or,
# where there is no twin (or another row already holds the twin's symbol), is taken out of trading.
# Availability is left alone on a repointed row: the next sync decides it from the live catalogue.
#
# A row something refers to is repointed like any other. Bots and transactions name a pair by its
# asset ids (and base/quote), bot_index_assets by the ticker row's id, which an in-place update
# keeps, and an order is tracked by its exchange order id rather than the symbol.
#
# Raw SQL: the app models keep changing.
class RepointGeminiPerpTickersToSpot < ActiveRecord::Migration[8.1]
  # Gemini's spot pairs as its API described them on 2026-09-21, keyed by the perpetual symbol a row
  # may hold. Frozen here rather than read from the seed file: a migration is a snapshot.
  #                     spot symbol   minimum_base_size  base/quote/price decimals, trading_enabled
  SPOT_TWINS = {
    'avaxgusdperp'   => ['avaxgusd',   '0.00499999', 6, 3, 3, true],
    'avaxusdcperp'   => ['avaxusdc',   '0.00499999', 6, 3, 3, true],
    'bchgusdperp'    => ['bchgusd',    '0.001',      6, 2, 2, true],
    'bchusdcperp'    => ['bchusdc',    '0.001',      6, 2, 2, true],
    'bnbgusdperp'    => ['bnbgusd',    '0.0002',     6, 4, 4, true],
    'bnbusdcperp'    => ['bnbusdc',    '0.0002',     6, 4, 4, true],
    'bonkgusdperp'   => ['bonkgusd',   '4000.0',     6, 9, 9, true],
    'bonkusdcperp'   => ['bonkusdc',   '4000.0',     6, 9, 9, true],
    'btcgusdperp'    => ['btcgusd',    '0.00001',    8, 2, 2, true],
    'btcusdcperp'    => ['btcusdc',    '0.00001',    8, 2, 2, true],
    'dogegusdperp'   => ['dogegusd',   '0.1',        6, 5, 5, true],
    'dogeusdcperp'   => ['dogeusdc',   '0.1',        6, 5, 5, true],
    'ethgusdperp'    => ['ethgusd',    '0.001',      6, 2, 2, true],
    'ethusdcperp'    => ['ethusdc',    '0.001',      6, 2, 2, true],
    'hypegusdperp'   => ['hypegusd',   '0.002',      6, 4, 4, true],
    'hypeusdcperp'   => ['hypeusdc',   '0.002',      6, 4, 4, true],
    'injgusdperp'    => ['injgusd',    '0.01',       6, 4, 4, true],
    'injusdcperp'    => ['injusdc',    '0.01',       6, 4, 4, true],
    'linkgusdperp'   => ['linkgusd',   '0.1',        6, 5, 5, true],
    'linkusdcperp'   => ['linkusdc',   '0.1',        6, 5, 5, true],
    'ltcgusdperp'    => ['ltcgusd',    '0.01',       5, 2, 2, true],
    'ltcusdcperp'    => ['ltcusdc',    '0.01',       5, 2, 2, true],
    'opgusdperp'     => ['opgusd',     '0.07',       6, 4, 4, true],
    'opusdcperp'     => ['opusdc',     '0.07',       6, 4, 4, true],
    'pepegusdperp'   => ['pepegusd',   '1000.0',     6, 9, 9, true],
    'pepeusdcperp'   => ['pepeusdc',   '1000.0',     6, 9, 9, true],
    'polgusdperp'    => ['polgusd',    '0.4',        6, 6, 6, true],
    'polusdcperp'    => ['polusdc',    '0.4',        6, 6, 6, true],
    'popcatgusdperp' => ['popcatgusd', '0.07',       6, 4, 4, true],
    'popcatusdcperp' => ['popcatusdc', '0.07',       6, 4, 4, true],
    'shibgusdperp'   => ['shibgusd',   '1000.0',     6, 9, 9, true],
    'shibusdcperp'   => ['shibusdc',   '1000.0',     6, 9, 9, true],
    'solgusdperp'    => ['solgusd',    '0.001',      6, 3, 3, true],
    'solusdcperp'    => ['solusdc',    '0.001',      6, 3, 3, true],
    'trumpgusdperp'  => ['trumpgusd',  '0.01',       6, 4, 4, true],
    'unigusdperp'    => ['unigusd',    '0.01',       6, 4, 4, true],
    'uniusdcperp'    => ['uniusdc',    '0.01',       6, 4, 4, true],
    'wifgusdperp'    => ['wifgusd',    '0.07',       6, 4, 4, true],
    'wifusdcperp'    => ['wifusdc',    '0.07',       6, 4, 4, true],
    'xrpgusdperp'    => ['xrpgusd',    '0.1',        6, 5, 5, true],
    'xrpusdcperp'    => ['xrpusdc',    '0.1',        6, 5, 5, true]
  }.freeze

  def up
    exchange_id = select_value("SELECT id FROM exchanges WHERE type = 'Exchanges::Gemini'")
    return if exchange_id.blank?

    perps = select_all("SELECT id, ticker FROM tickers WHERE exchange_id = #{exchange_id} AND lower(ticker) LIKE '%perp'")
    perps.each do |row|
      twin = SPOT_TWINS[row['ticker']]
      if twin && !symbol_taken?(exchange_id, twin.first, row['id'])
        repoint(row['id'], *twin)
      else
        disable(row['id'])
      end
    end
  end

  def down
    raise ActiveRecord::IrreversibleMigration
  end

  private

  def symbol_taken?(exchange_id, symbol, own_id)
    select_value(<<~SQL.squish).present?
      SELECT 1 FROM tickers
      WHERE exchange_id = #{exchange_id} AND ticker = #{quote(symbol)} AND id != #{own_id.to_i}
      LIMIT 1
    SQL
  end

  def repoint(id, symbol, minimum_base_size, base_decimals, quote_decimals, price_decimals, trading_enabled)
    execute(<<~SQL.squish)
      UPDATE tickers
      SET ticker = #{quote(symbol)}, minimum_base_size = #{quote(BigDecimal(minimum_base_size))},
          base_decimals = #{base_decimals}, quote_decimals = #{quote_decimals}, price_decimals = #{price_decimals},
          trading_enabled = #{quote(trading_enabled)}, updated_at = CURRENT_TIMESTAMP
      WHERE id = #{id.to_i}
    SQL
  end

  # Guarded so a second run, which finds the same rows, writes nothing.
  def disable(id)
    execute(<<~SQL.squish)
      UPDATE tickers
      SET available = 0, trading_enabled = 0, updated_at = CURRENT_TIMESTAMP
      WHERE id = #{id.to_i} AND (available IS NOT 0 OR trading_enabled IS NOT 0)
    SQL
  end

  def quote(value)
    ActiveRecord::Base.connection.quote(value)
  end
end
