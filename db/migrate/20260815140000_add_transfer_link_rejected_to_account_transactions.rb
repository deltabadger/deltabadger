class AddTransferLinkRejectedToAccountTransactions < ActiveRecord::Migration[8.1]
  def change
    add_column :account_transactions, :transfer_link_rejected, :boolean, default: false, null: false
  end
end
