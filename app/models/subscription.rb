class Subscription < ApplicationRecord
  include Legendary

  belongs_to :subscription_plan
  belongs_to :user

  scope :current, -> { where('end_time > ?', Time.now) }

  delegate :name, to: :subscription_plan
  delegate :display_name, to: :subscription_plan
  delegate :unlimited?, to: :subscription_plan

  def self.number_of_active_subscriptions(name)
    0 || grouped_subscriptions_cache[name]
  end

  def self.grouped_subscriptions_cache
    @grouped_subscriptions_cache ||= joins(:subscription_plan)
                                     .merge(SubscriptionPlan.all)
                                     .current
                                     .group('subscription_plans.name')
                                     .count
  end
end
