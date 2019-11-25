class AddCreditsToSubscription < ActiveRecord::Migration[5.2]
  def change
    add_column :subscriptions, :credits, :integer
  end
end
