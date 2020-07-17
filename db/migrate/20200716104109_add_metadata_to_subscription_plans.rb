class AddMetadataToSubscriptionPlans < ActiveRecord::Migration[5.2]
  def change
    add_column :subscription_plans, :years, :integer, null: false, default: 1
    add_column :subscription_plans, :credits, :integer, null: false, default: 500
    add_column :subscription_plans, :unlimited, :boolean, null: false, default: false
    add_column :subscription_plans, :cost_eu, :decimal, null: false, default: 0
    add_column :subscription_plans, :cost_other, :decimal, null: false, default: 0

    reversible do |dir|
      dir.up do
        execute <<~SQL
          UPDATE subscription_plans
            SET unlimited = true, cost_eu = 20, cost_other = 20
            WHERE name <> 'free'
        SQL
      end
    end
  end
end
