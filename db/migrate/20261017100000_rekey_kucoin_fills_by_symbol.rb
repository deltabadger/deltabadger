# KuCoin fills were stored under the bare tradeId, which KuCoin numbers per symbol, so a fill on one
# market and a fill on another with the same tradeId shared a tx_id and the sync kept only one of them.
# The sync now keys a fill "<symbol>-<tradeId>". This rewrites the held rows to that key, from the
# symbol and tradeId raw_data keeps (it is the fill as received), so a re-read finds them.
#
# Only rows in the API's shape (raw_data has tradeId and symbol) are touched; a file-imported row keeps
# its id. A row is never moved onto an id another row of that user already holds. updated_at moves with
# them, so the tracker's ledger cache (keyed on the newest updated_at) re-reads.
#
# The fills a collision dropped were never stored, so the watermark of every KuCoin key goes back to nil:
# the next scheduled sync reads the full history, and the rows already held come back as duplicates.
class RekeyKucoinFillsBySymbol < ActiveRecord::Migration[8.1]
  FILLS = [0, 1].freeze # AccountTransaction entry_type: buy, sell

  def up
    exchange_ids = select_values("SELECT id FROM exchanges WHERE type = 'Exchanges::Kucoin'")
    return if exchange_ids.empty?

    now = connection.quote(Time.current.utc)
    key = "json_extract(t.raw_data, '$.symbol') || '-' || json_extract(t.raw_data, '$.tradeId')"
    execute(<<~SQL.squish)
      UPDATE account_transactions AS t
      SET tx_id = #{key}, updated_at = #{now}
      WHERE t.exchange_id IN (#{exchange_ids.join(', ')})
        AND t.entry_type IN (#{FILLS.join(', ')})
        AND json_extract(t.raw_data, '$.tradeId') IS NOT NULL AND json_extract(t.raw_data, '$.tradeId') != ''
        AND json_extract(t.raw_data, '$.symbol') IS NOT NULL AND json_extract(t.raw_data, '$.symbol') != ''
        AND t.tx_id IS NOT #{key}
        AND NOT EXISTS (
          SELECT 1 FROM account_transactions AS o
          WHERE o.user_id = t.user_id AND o.exchange_id = t.exchange_id AND o.tx_id = #{key}
        )
    SQL
    execute("UPDATE api_keys SET last_synced_at = NULL WHERE exchange_id IN (#{exchange_ids.join(', ')})")
  end

  def down
    # Nothing to restore: the bare tradeId was the defect, and the sync keys fills by symbol from now on.
  end
end
