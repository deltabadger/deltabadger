module SubscriptionPlan::PlanStats
  extend ActiveSupport::Concern

  included do
    def active_subscriptions_count
      @active_subscriptions_count ||= Subscription.active.by_plan_name(name)&.count || 0
    end

    def total_supply
      ensure_legendary!
      LegendaryBadgersCollection::TOTAL_SUPPLY
    end

    def sold_percent
      ensure_legendary!
      return 0 if total_supply.zero?

      active_subscriptions_count * 100 / total_supply
    end

    def for_sale_count
      ensure_legendary!
      total_supply - active_subscriptions_count
    end

    def available?
      ensure_legendary!
      (active_subscriptions_count >= 0) && (active_subscriptions_count < total_supply)
    end

    private

    def ensure_legendary!
      raise 'This method is only available for the Legendary Badger NFT plan' unless legendary?
    end
  end
end
