class AddForeignKeyToUsersForPendingPlan < ActiveRecord::Migration[6.0]
  def change
    add_foreign_key :users, :subscription_plan_variants, column: :pending_plan_id
    rename_column :users, :pending_plan_id, :pending_plan_variant_id
  end
end
