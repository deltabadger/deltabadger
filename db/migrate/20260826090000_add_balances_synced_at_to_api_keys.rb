# The balance rows cannot carry a venue's sync watermark — a successful empty sync deletes them —
# so the key does. Backfilled from the rows that exist, so an account synced before this is not
# read as never synced. The ledgers open each asset with what must have been held before its
# history begins, which changes every day of a stored history: rebuilt once, as before.
class AddBalancesSyncedAtToApiKeys < ActiveRecord::Migration[8.1]
  def up
    add_column :api_keys, :balances_synced_at, :datetime
    execute <<~SQL.squish
      UPDATE api_keys SET balances_synced_at = (
        SELECT MAX(account_balances.synced_at) FROM account_balances
        WHERE account_balances.user_id = api_keys.user_id AND account_balances.exchange_id = api_keys.exchange_id
      )
    SQL
    AccountTransaction.distinct.pluck(:user_id).each do |user_id|
      PortfolioSnapshot::BackfillJob.perform_later(user_id)
      Tracker::LedgerJob.perform_later(user_id)
    end
  end

  def down
    remove_column :api_keys, :balances_synced_at
  end
end
