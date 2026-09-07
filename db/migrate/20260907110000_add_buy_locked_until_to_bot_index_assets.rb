class AddBuyLockedUntilToBotIndexAssets < ActiveRecord::Migration[8.1]
  def change
    # The effective lock, and the part of it a FILL confirmed. The effective one carries provisional
    # placement-time locks too and can be rolled back to a previous value; the confirmed one is only
    # ever raised, and a rollback can never go below it.
    add_column :bot_index_assets, :buy_locked_until, :datetime
    add_column :bot_index_assets, :confirmed_locked_until, :datetime
  end
end
