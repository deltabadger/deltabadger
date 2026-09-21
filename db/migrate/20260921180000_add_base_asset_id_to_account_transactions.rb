# Ledger rows record the asset they moved, not only its symbol: a symbol can name a coin on one venue
# and a stock on another, and a fully sold position has no balance left to say which it was. Existing
# rows are resolved by a job — the rule needs each venue's adapter and its listings; a row that stays
# NULL is read by its symbol, as before.
class AddBaseAssetIdToAccountTransactions < ActiveRecord::Migration[8.1]
  def up
    add_column :account_transactions, :base_asset_id, :integer

    AccountTransaction.distinct.pluck(:user_id).each do |user_id|
      AccountTransaction::ResolveAssetsJob.perform_later(user_id)
    end
  end

  def down
    remove_column :account_transactions, :base_asset_id
  end
end
