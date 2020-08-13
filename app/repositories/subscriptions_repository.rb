class SubscriptionsRepository < BaseRepository
  def model
    Subscription
  end

  def all_current_count(name)
    model
      .joins(:subscription_plan)
      .merge(SubscriptionPlan
      .where(name: name))
      .current
      .count
  end
end
