class MakeTransactionsExchangeIdNotNull < ActiveRecord::Migration[6.0]
  def change
    change_column_null :transactions, :exchange_id, false
  end
end
