class AddSubscriptionPlanIdToPayments < ActiveRecord::Migration[5.2]
  def change
    add_reference :payments, :subscription_plan

    reversible do |dir|
      dir.up do
        execute <<~SQL
          UPDATE payments
            SET subscription_plan_id =
              (SELECT id FROM subscription_plans WHERE name = 'unlimited')
        SQL
      end
    end

    change_column_null :payments, :subscription_plan_id, false
    add_foreign_key :payments, :subscription_plans
  end
end
