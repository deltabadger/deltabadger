class AddDisplayCurrencyToUsers < ActiveRecord::Migration[8.1]
  def change
    add_column :users, :display_currency, :string, null: false, default: 'USD'
  end
end
