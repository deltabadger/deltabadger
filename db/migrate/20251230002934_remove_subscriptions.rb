class RemoveSubscriptions < ActiveRecord::Migration[6.0]
  def change
    remove_column :users, :pending_plan_variant_id
    drop_table :subscriptions
    drop_table :subscription_plan_variants
    drop_table :subscription_plans
  end
end
