class MigrateSubscriptionPlanData < ActiveRecord::Migration[6.0]
  def up
    # Create new subscription plan variants
    SubscriptionPlan.find_each do |record|
      SubscriptionPlanVariant.create!(
        subscription_plan_id: record.id,
        years: record.years > 1000 ? nil : record.years,
        cost_eur: record.cost_eu,
        cost_usd: record.cost_other
      )
    end

    remove_column :subscription_plans, :years
    remove_column :subscription_plans, :cost_eu
    remove_column :subscription_plans, :cost_other

    plan_ids_map = {} # subscription_plan_id => subscription_plan_variant_id
    SubscriptionPlanVariant.find_each do |record|
      plan_ids_map[record.subscription_plan_id] = record.id
    end

    # Associate payments with the new subscription plan variants
    remove_foreign_key :payments, :subscription_plans
    rename_column :payments, :subscription_plan_id, :subscription_plan_variant_id
    Payment.find_each do |record|
      record.update_columns(subscription_plan_variant_id: plan_ids_map[record.subscription_plan_variant_id])
    end
    add_foreign_key :payments, :subscription_plan_variants

    # Associate subscriptions with the new subscription plan variants
    remove_foreign_key :subscriptions, :subscription_plans
    rename_column :subscriptions, :subscription_plan_id, :subscription_plan_variant_id
    Subscription.find_each do |record|
      record.update_columns(end_time: nil) if record.end_time.present? && record.end_time > Time.current + 1000.years
      record.update_columns(subscription_plan_variant_id: plan_ids_map[record.subscription_plan_variant_id])
    end
    add_foreign_key :subscriptions, :subscription_plan_variants
  end

  def down
    plan_ids_map = {} # subscription_plan_variant_id => subscription_plan_id
    SubscriptionPlanVariant.find_each do |record|
      plan_ids_map[record.id] = record.subscription_plan_id
    end

    if plan_ids_map.keys.uniq.length != plan_ids_map.keys.length
      raise 'Unable to migrate data back, multiple subscription plan variants have the same subscription plan id and payments and subscripitions would be incorrectly associated'
    end

    # Associate payments with the old subscription plans
    remove_foreign_key :payments, :subscription_plan_variants
    rename_column :payments, :subscription_plan_variant_id, :subscription_plan_id
    Payment.find_each do |record|
      record.update_columns(subscription_plan_id: plan_ids_map[record.subscription_plan_id])
    end
    add_foreign_key :payments, :subscription_plans

    # Associate subscriptions with the old subscription plans
    remove_foreign_key :subscriptions, :subscription_plan_variants
    rename_column :subscriptions, :subscription_plan_variant_id, :subscription_plan_id
    Subscription.find_each do |record|
      record.update_columns(end_time: Time.current + 10000.years) if record.end_time.nil?
      record.update_columns(subscription_plan_id: plan_ids_map[record.subscription_plan_id])
    end
    add_foreign_key :subscriptions, :subscription_plans

    add_column :subscription_plans, :years, :integer, default: 1, null: false
    add_column :subscription_plans, :cost_eu, :decimal, default: 0.0, null: false
    add_column :subscription_plans, :cost_other, :decimal, default: 0.0, null: false

    SubscriptionPlan.reset_column_information

    grouped_variants = SubscriptionPlanVariant.all.group_by(&:subscription_plan_id)
    grouped_variants.each do |subscription_plan_id, subscription_plan_variants|
      record = SubscriptionPlan.find(subscription_plan_id)
      variant_data = subscription_plan_variants.first
      record.update_columns(years: variant_data['years'] || 10000, cost_eu: variant_data['cost_eur'], cost_other: variant_data['cost_usd'])
    end

    SubscriptionPlanVariant.delete_all
  end
end
