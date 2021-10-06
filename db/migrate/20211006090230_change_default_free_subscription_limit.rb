class ChangeDefaultFreeSubscriptionLimit < ActiveRecord::Migration[5.2]
  def change
    change_column_default :subscription_plans, :credits, 1200
  end
end
