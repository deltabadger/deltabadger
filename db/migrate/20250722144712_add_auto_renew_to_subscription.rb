class AddAutoRenewToSubscription < ActiveRecord::Migration[6.0]
  def change
    add_column :subscriptions, :auto_renew, :boolean, default: false
  end
end
