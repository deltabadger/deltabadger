class MigrateSubscriptionPlanData < ActiveRecord::Migration[6.0]
  def up
    SubscriptionPlan.find_each do |record|
      SubscriptionPlanVariant.create!(
        subscription_plan_id: record.id,
        years: record.years,
        cost_eur: record.cost_eu,
        cost_usd: record.cost_other
      )
    end

    remove_column :subscription_plans, :years
    remove_column :subscription_plans, :cost_eu
    remove_column :subscription_plans, :cost_other
  end

  def down
    add_column :subscription_plans, :years, :integer, default: 1, null: false
    add_column :subscription_plans, :cost_eu, :decimal, default: 0.0, null: false
    add_column :subscription_plans, :cost_other, :decimal, default: 0.0, null: false

    SubscriptionPlan.reset_column_information

    grouped_variants = SubscriptionPlanVariant.all.group_by(&:subscription_plan_id)
    grouped_variants.each do |subscription_plan_id, subscription_plan_variants|
      record = SubscriptionPlan.find(subscription_plan_id)
      variant_data = subscription_plan_variants.first
      record.update_columns(years: variant_data['years'], cost_eu: variant_data['cost_eur'], cost_other: variant_data['cost_usd'])
    end

    SubscriptionPlanVariant.delete_all
  end
end
