# One-time backfill of transactions.base_asset_id / quote_asset_id for rows recorded before orders stored
# their assets. A row carries only symbol strings, and a string can name several assets: asset symbols are
# not unique on one venue (MEXC's POR is two assets), a venue may spell an asset its own way (Kraken's XBT),
# and symbols get renamed. So a row is resolved from what its bot could have traded, and left NULL whenever
# more than one asset fits — a NULL row is read by its string, as every row was before.
#
# Raw SQL, no model: it runs inside a migration, where a bot row of a retired type cannot be instantiated and
# app code of older migrations must not see these columns. Idempotent: an id already on a row is never
# overwritten, and a second run finds nothing to do.
#
# Rules, per (bot, exchange, base, quote, recorded quote id, transaction type, imported or placed):
# - quote: the row's own id when set; else the bot's quote asset when the string is its symbol or its spelling
#   on the row's exchange; else the one asset the venue quotes under that name.
# - single-asset and signal bots: their one base asset — they trade a single pair, fixed once they have orders.
# - baskets: the one member (allocations and every membership, exited included) the string names on the
#   row's exchange, by symbol or venue spelling.
# - index bots, regular and redeploy rows the bot placed: the same member rule. A row imported from a CSV may
#   be any asset the venue lists, so it takes the one asset its members and the venue together know by
#   that name, else stays NULL. Rebalance and liquidation rows picked their
#   ticker by matching a string against the venue's SPELLINGS across the whole venue and recorded the traded
#   asset's symbol, so any asset with that symbol whose ticker is spelled like a string the leg could have
#   routed by (a member's symbol; for a liquidation, also a string the bot held) is a candidate. With no
#   candidate at all, the one asset the venue spells or symbols that way at the row's quote.
# Symbols are compared as they are now: a rename since the row can leave it unresolved.
module Transaction::AssetBackfill
  module_function

  INDEX = 'Bots::DcaIndex'.freeze
  BASKET = 'Bots::DcaMultiAsset'.freeze
  SINGLE_PAIR_TYPES = %w[Bots::DcaSingleAsset Bots::Signal].freeze
  ROUTED_TYPES = %w[REBALANCE LIQUIDATION].freeze

  # @return [Hash] rows resolved per rule, and what is left NULL
  def run!(connection)
    @connection = connection
    drop_temp_tables
    build_universe
    build_combos
    resolve_quotes
    resolve_single_pairs
    resolve_members
    resolve_imported
    resolve_routed
    resolve_by_venue
    apply
    report
  ensure
    drop_temp_tables
  end

  # A ticker's venue spelling, upper-cased, without a data-api tombstone prefix (__stale_<id>_).
  # Whether a row came from a CSV import rather than from an order the bot placed.
  def imported(column) = "CASE WHEN #{column} LIKE 'imported\\_%' ESCAPE '\\' THEN 1 ELSE 0 END"

  def spelling(column)
    "upper(CASE WHEN #{column} LIKE '\\_\\_stale\\_%' ESCAPE '\\' " \
      "THEN substr(substr(#{column}, 9), instr(substr(#{column}, 9), '_') + 1) ELSE #{column} END)"
  end

  def execute(sql) = @connection.execute(sql)

  def drop_temp_tables
    %w[backfill_universe backfill_combos backfill_routes backfill_held].each do |table|
      execute("DROP TABLE IF EXISTS temp.#{table}")
    end
  end

  # What each basket or index bot could have traded: its allocations and every membership, exited included.
  def build_universe
    execute(<<~SQL)
      CREATE TEMP TABLE backfill_universe (bot_id INTEGER, asset_id INTEGER, PRIMARY KEY (bot_id, asset_id)) WITHOUT ROWID
    SQL
    execute(<<~SQL)
      INSERT OR IGNORE INTO backfill_universe
      SELECT b.id, CAST(je.key AS INTEGER)
        FROM bots b, json_each(CASE WHEN json_valid(b.settings) THEN b.settings ELSE '{}' END, '$.allocations') je
       WHERE b.type = '#{BASKET}'
    SQL
    execute('INSERT OR IGNORE INTO backfill_universe SELECT bot_id, asset_id FROM bot_index_assets')
  end

  def build_combos
    execute(<<~SQL)
      CREATE TEMP TABLE backfill_combos AS
      SELECT t.bot_id, t.exchange_id, t.base, t.quote, t.quote_asset_id AS recorded_quote, t.transaction_type,
             #{imported('t.external_id')} AS imported, b.type AS bot_type,
             CASE WHEN json_valid(b.settings) THEN json_extract(b.settings, '$.quote_asset_id') END AS bot_quote,
             CASE WHEN json_valid(b.settings) THEN json_extract(b.settings, '$.base_asset_id') END AS bot_base,
             count(*) AS n, NULL AS q, 0 AS h, NULL AS b, NULL AS rule
        FROM transactions t JOIN bots b ON b.id = t.bot_id
       WHERE t.base_asset_id IS NULL OR t.quote_asset_id IS NULL
       GROUP BY t.bot_id, t.exchange_id, t.base, t.quote, t.quote_asset_id, t.transaction_type, imported
    SQL
  end

  def resolve_quotes
    execute('UPDATE backfill_combos SET q = recorded_quote WHERE recorded_quote IS NOT NULL')
    execute(<<~SQL)
      UPDATE backfill_combos SET q = (
        SELECT a.id FROM assets a
         WHERE a.id = backfill_combos.bot_quote
           AND (upper(a.symbol) = upper(backfill_combos.quote)
                OR EXISTS (SELECT 1 FROM tickers t WHERE t.exchange_id = backfill_combos.exchange_id
                              AND t.quote_asset_id = a.id AND upper(t.quote) = upper(backfill_combos.quote))))
       WHERE q IS NULL
    SQL
    execute(<<~SQL)
      UPDATE backfill_combos SET q = (
        SELECT CASE WHEN count(DISTINCT t.quote_asset_id) = 1 THEN max(t.quote_asset_id) END
          FROM tickers t JOIN assets a ON a.id = t.quote_asset_id
         WHERE t.exchange_id = backfill_combos.exchange_id
           AND (upper(t.quote) = upper(backfill_combos.quote) OR upper(a.symbol) = upper(backfill_combos.quote)))
       WHERE q IS NULL
    SQL
  end

  def resolve_single_pairs
    execute(<<~SQL)
      UPDATE backfill_combos SET h = 1, b = bot_base, rule = 'pair'
       WHERE bot_type IN (#{quoted(SINGLE_PAIR_TYPES)}) AND bot_base IS NOT NULL
         AND EXISTS (SELECT 1 FROM assets WHERE id = backfill_combos.bot_base)
    SQL
  end

  def resolve_members
    execute(<<~SQL)
      UPDATE backfill_combos SET rule = 'member', (h, b) = (
        SELECT count(DISTINCT u.asset_id), max(u.asset_id)
          FROM backfill_universe u JOIN assets a ON a.id = u.asset_id
         WHERE u.bot_id = backfill_combos.bot_id
           AND (upper(a.symbol) = upper(backfill_combos.base)
                OR EXISTS (SELECT 1 FROM tickers t WHERE t.exchange_id = backfill_combos.exchange_id
                              AND t.base_asset_id = u.asset_id AND #{spelling('t.base')} = upper(backfill_combos.base))))
       WHERE bot_type = '#{BASKET}'
          OR (bot_type = '#{INDEX}' AND imported = 0 AND transaction_type NOT IN (#{quoted(ROUTED_TYPES)}))
    SQL
  end

  # An index bot's imported row: every asset named that way that is a member or listed at the row's quote.
  def resolve_imported
    execute(<<~SQL)
      UPDATE backfill_combos SET rule = 'imported', (h, b) = (
        SELECT count(DISTINCT a.id), max(a.id) FROM assets a
         WHERE (upper(a.symbol) = upper(backfill_combos.base)
                OR a.id IN (SELECT t.base_asset_id FROM tickers t WHERE t.exchange_id = backfill_combos.exchange_id
                              AND #{spelling('t.base')} = upper(backfill_combos.base)))
           AND (EXISTS (SELECT 1 FROM tickers t WHERE t.exchange_id = backfill_combos.exchange_id
                          AND t.quote_asset_id = backfill_combos.q AND t.base_asset_id = a.id)
                OR EXISTS (SELECT 1 FROM backfill_universe u WHERE u.bot_id = backfill_combos.bot_id AND u.asset_id = a.id)))
       WHERE bot_type = '#{INDEX}' AND imported = 1 AND transaction_type NOT IN (#{quoted(ROUTED_TYPES)})
    SQL
  end

  def resolve_routed
    execute('CREATE TEMP TABLE backfill_routes (bot_id INTEGER, string TEXT, PRIMARY KEY (bot_id, string)) WITHOUT ROWID')
    execute(<<~SQL)
      INSERT OR IGNORE INTO backfill_routes
      SELECT u.bot_id, upper(a.symbol) FROM backfill_universe u JOIN assets a ON a.id = u.asset_id
        JOIN bots b ON b.id = u.bot_id
       WHERE b.type = '#{INDEX}' AND a.symbol IS NOT NULL
    SQL
    execute('CREATE TEMP TABLE backfill_held (bot_id INTEGER, string TEXT, PRIMARY KEY (bot_id, string)) WITHOUT ROWID')
    execute(<<~SQL)
      INSERT OR IGNORE INTO backfill_held
      SELECT t.bot_id, upper(t.base) FROM transactions t JOIN bots b ON b.id = t.bot_id
       WHERE b.type = '#{INDEX}' AND t.base IS NOT NULL
    SQL
    execute(<<~SQL)
      UPDATE backfill_combos SET rule = 'routed', (h, b) = (
        SELECT count(DISTINCT a.id), max(a.id)
          FROM assets a JOIN tickers t ON t.base_asset_id = a.id
         WHERE upper(a.symbol) = upper(backfill_combos.base)
           AND t.exchange_id = backfill_combos.exchange_id AND t.quote_asset_id = backfill_combos.q
           AND (#{spelling('t.base')} IN (SELECT string FROM backfill_routes r WHERE r.bot_id = backfill_combos.bot_id)
                OR (backfill_combos.transaction_type = 'LIQUIDATION'
                    AND #{spelling('t.base')} IN (SELECT string FROM backfill_held hh WHERE hh.bot_id = backfill_combos.bot_id))))
       WHERE bot_type = '#{INDEX}' AND transaction_type IN (#{quoted(ROUTED_TYPES)})
    SQL
  end

  # An index bot's row that no candidate above explains: the one asset the venue spells or symbols that way.
  def resolve_by_venue
    execute(<<~SQL)
      UPDATE backfill_combos SET rule = 'venue', (h, b) = (
        SELECT count(DISTINCT t.base_asset_id), max(t.base_asset_id)
          FROM tickers t JOIN assets a ON a.id = t.base_asset_id
         WHERE t.exchange_id = backfill_combos.exchange_id AND t.quote_asset_id = backfill_combos.q
           AND (#{spelling('t.base')} = upper(backfill_combos.base) OR upper(a.symbol) = upper(backfill_combos.base)))
       WHERE bot_type = '#{INDEX}' AND h = 0 AND imported = 0
    SQL
  end

  def apply
    execute(<<~SQL)
      UPDATE transactions
         SET base_asset_id = coalesce(transactions.base_asset_id, CASE WHEN c.h = 1 THEN c.b END),
             quote_asset_id = coalesce(transactions.quote_asset_id, c.q)
        FROM backfill_combos c
       WHERE transactions.bot_id = c.bot_id AND transactions.exchange_id = c.exchange_id
         AND transactions.base IS c.base AND transactions.quote IS c.quote
         AND transactions.quote_asset_id IS c.recorded_quote
         AND transactions.transaction_type = c.transaction_type
         AND #{imported('transactions.external_id')} = c.imported
         AND (transactions.base_asset_id IS NULL OR transactions.quote_asset_id IS NULL)
    SQL
  end

  def report
    resolved = @connection.select_rows(<<~SQL).to_h { |rule, rows| [rule, rows.to_i] }
      SELECT rule, sum(n) FROM backfill_combos WHERE h = 1 GROUP BY rule
    SQL
    unresolved = @connection.select_rows(<<~SQL)
      SELECT bot_id, exchange_id, base, quote, h, n FROM backfill_combos WHERE h != 1 OR h IS NULL ORDER BY n DESC
    SQL
    {
      resolved:,
      unresolved_rows: @connection.select_value('SELECT count(*) FROM transactions WHERE base_asset_id IS NULL').to_i,
      unresolved_combos: unresolved
    }
  end

  def quoted(values) = values.map { |value| "'#{value}'" }.join(', ')
end
