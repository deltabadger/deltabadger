class CreateSubscriptionPlanVariants < ActiveRecord::Migration[6.0]
  def change
    create_table :subscription_plan_variants do |t|
      t.integer :subscription_plan_id, null: false, foreign_key: true
      t.integer :years
      t.decimal :cost_eur, precision: 10, scale: 2
      t.decimal :cost_usd, precision: 10, scale: 2

      t.timestamps
    end

    add_foreign_key :subscription_plan_variants, :subscription_plans
  end
end
