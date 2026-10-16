# Bitrue names assets in lower case, and its ledger sync stored them that way ("btc"/"usdt") while every
# other row, ticker and tax pool keys them upper case. The sync now upper-cases, but re-reads dedupe on
# tx_id and leave the old rows as they were, so an old "eth" buy and a new "ETH" sale sat in different
# FIFO pools and the sale lost its cost basis. This upper-cases them in place.
#
# updated_at moves with them: the tracker's ledger cache is keyed on the newest updated_at, so the
# rewrite is read at once. Rows still without a base asset are resolved now that their symbol matches.
#
# The watermark goes back to nil on every Bitrue key: the old sync read one page of recent trades and
# no deposits or withdrawals at all, so the next sync reads the full history again. Rows already held
# come back as duplicates and are skipped by tx_id.
#
# Irreversible in effect: which rows were lower case is not kept, and nothing reads them that way.
class UpcaseBitrueLedgerCurrencies < ActiveRecord::Migration[8.1]
  COLUMNS = %w[base_currency quote_currency fee_currency].freeze

  def up
    exchange_ids = select_values("SELECT id FROM exchanges WHERE type = 'Exchanges::Bitrue'")
    return if exchange_ids.empty?

    ids = exchange_ids.join(', ')
    now = connection.quote(Time.current.utc)
    COLUMNS.each do |column|
      execute(<<~SQL.squish)
        UPDATE account_transactions SET #{column} = UPPER(#{column}), updated_at = #{now}
        WHERE exchange_id IN (#{ids}) AND #{column} IS NOT NULL AND #{column} != UPPER(#{column})
      SQL
    end
    execute("UPDATE api_keys SET last_synced_at = NULL WHERE exchange_id IN (#{ids})")

    Exchange.where(id: exchange_ids).find_each do |exchange|
      AccountTransaction.resolve_base_assets!(AccountTransaction.where(exchange_id: exchange.id), exchange: exchange)
    end
  end

  def down
    # Nothing to restore: the lower case was the defect, and the next sync moves the watermark forward.
  end
end
