class SubscriptionsRepository < BaseRepository
  def model
    Subscription
  end

  def number_of_active_subscriptions(name)
    model
      .joins(:subscription_plan)
      .merge(SubscriptionPlan
      .where(name: name))
      .current
      .count
  end
end
