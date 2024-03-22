class SubscriptionsRepository < BaseRepository
  def model
    Subscription
  end

  def number_of_active_subscriptions(name)
    find_by_name!(name)
  end

  private

  def subscriptions_cache
    @subscriptions_cache ||= model
                             .joins(:subscription_plan)
                             .merge(SubscriptionPlan.all)
                             .current
                             .group('subscription_plans.name')
                             .count
  end

  def find_by_name!(name)
    subscriptions_cache.fetch(name)
  end
end
