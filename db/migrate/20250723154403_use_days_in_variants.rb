class UseDaysInVariants < ActiveRecord::Migration[6.0]
  def up
    rename_column :subscription_plan_variants, :years, :days
    SubscriptionPlanVariant.where(days: 0).update_all(days: 30)
    SubscriptionPlanVariant.where(days: 1).update_all(days: 365)
    SubscriptionPlanVariant.where(days: 4).update_all(days: 1460)
  end

  def down
    raise ActiveRecord::IrreversibleMigration if unknown_years?

    rename_column :subscription_plan_variants, :days, :years
    SubscriptionPlanVariant.where(days: 30).update_all(days: 0)
    SubscriptionPlanVariant.where(days: 365).update_all(days: 1)
    SubscriptionPlanVariant.where(days: 1460).update_all(days: 4)
  end
end

def unknown_years?
  SubscriptionPlanVariant.find_each do |variant|
    return true unless variant.days.in?([30, 365, 1460])
  end
  false
end
