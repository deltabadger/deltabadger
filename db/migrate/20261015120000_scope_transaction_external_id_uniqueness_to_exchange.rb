# An order id is unique on its venue only: Binance and Binance.US can hand out the same one,
# and the second order then failed to save.
class ScopeTransactionExternalIdUniquenessToExchange < ActiveRecord::Migration[8.1]
  def up
    # The new key is a superset of the old one, so it cannot reject an existing row.
    remove_index :transactions, name: :index_transactions_on_external_id
    add_index :transactions, %i[exchange_id external_id], unique: true
  end

  # Best effort. The old key is narrower: two venues' rows with the same id since make this
  # raise, and they have to be reconciled by hand.
  def down
    remove_index :transactions, column: %i[exchange_id external_id]
    add_index :transactions, :external_id, unique: true
  end
end
