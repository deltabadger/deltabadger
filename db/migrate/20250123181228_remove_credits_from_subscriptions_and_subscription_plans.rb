class RemoveCreditsFromSubscriptionsAndSubscriptionPlans < ActiveRecord::Migration[6.0]
  def change
    remove_column :subscription_plans, :credits, :integer, default: 100_000
    remove_column :subscriptions, :credits, :integer, default: 100_000
    remove_column :subscriptions, :limit_almost_reached_sent, :boolean, default: false
    remove_column :subscriptions, :first_month_ending_sent_at, :datetime
  end
end
