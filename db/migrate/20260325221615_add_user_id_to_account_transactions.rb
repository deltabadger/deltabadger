class AddUserIdToAccountTransactions < ActiveRecord::Migration[8.1]
  def up
    add_reference :account_transactions, :user, null: true, foreign_key: true

    execute <<~SQL
      UPDATE account_transactions
      SET user_id = (SELECT user_id FROM api_keys WHERE api_keys.id = account_transactions.api_key_id)
    SQL

    change_column_null :account_transactions, :user_id, false
    change_column_null :account_transactions, :api_key_id, true
  end

  def down
    change_column_null :account_transactions, :api_key_id, false
    remove_reference :account_transactions, :user
  end
end
