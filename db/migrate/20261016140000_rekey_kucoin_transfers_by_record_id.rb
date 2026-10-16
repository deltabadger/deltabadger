# KuCoin deposits and withdrawals were stored under walletTxId, the on-chain hash, which every transfer
# batched into one transaction shares (an internal transfer has none). The sync now keys them by the
# record's own `id`, so a re-read of a transfer already held would miss the old row and store it again.
# This rewrites the held rows to the id KuCoin sent with them, which raw_data keeps (it is the record as
# received).
#
# Only rows in the API's shape (raw_data has a walletTxId key) are touched; a file-imported row keeps its
# id. A row whose record carried no `id` keeps its hash: the adapter keys an id-less record by its hash
# too, so a re-read still finds it. A row is never moved onto an id another row of that user already holds.
#
# updated_at moves with them, so the tracker's ledger cache (keyed on the newest updated_at) re-reads.
class RekeyKucoinTransfersByRecordId < ActiveRecord::Migration[8.1]
  TRANSFERS = [4, 5].freeze # AccountTransaction entry_type: deposit, withdrawal

  def up
    exchange_ids = select_values("SELECT id FROM exchanges WHERE type = 'Exchanges::Kucoin'")
    return if exchange_ids.empty?

    now = connection.quote(Time.current.utc)
    execute(<<~SQL.squish)
      UPDATE account_transactions AS t
      SET tx_id = json_extract(t.raw_data, '$.id'), updated_at = #{now}
      WHERE t.exchange_id IN (#{exchange_ids.join(', ')})
        AND t.entry_type IN (#{TRANSFERS.join(', ')})
        AND json_type(t.raw_data, '$.walletTxId') IS NOT NULL
        AND json_extract(t.raw_data, '$.id') IS NOT NULL AND json_extract(t.raw_data, '$.id') != ''
        AND t.tx_id IS NOT json_extract(t.raw_data, '$.id')
        AND NOT EXISTS (
          SELECT 1 FROM account_transactions AS o
          WHERE o.user_id = t.user_id AND o.exchange_id = t.exchange_id AND o.tx_id = json_extract(t.raw_data, '$.id')
        )
    SQL
  end

  def down
    # Nothing to restore: the hash was the defect, and the sync keys these rows by id from now on.
  end
end
