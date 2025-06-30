class RemoveUnlimitedFromSubscriptionPlans < ActiveRecord::Migration[6.0]
  def change
    remove_column :subscription_plans, :unlimited, :boolean, default: false
  end
end
