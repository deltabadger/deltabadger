class SubscriptionsRepository < BaseRepository
  def model
    Subscription
  end

  def all_current_unlimited_count
    model
      .joins(:subscription_plan)
      .merge(SubscriptionPlan
      .where(name: 'unlimited'))
      .current
      .count
  end
end
