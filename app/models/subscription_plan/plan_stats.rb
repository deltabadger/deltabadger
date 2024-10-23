module SubscriptionPlan::PlanStats
  extend ActiveSupport::Concern

  included do # rubocop:disable Metrics/BlockLength
    def active_subscriptions_count
      @active_subscriptions_count ||= Subscription.active.by_plan_name(name)&.count || 0
    end

    def current_discount
      ensure_legendary!
      [0, plans_for_sale_count * 10].max
    end

    def total_supply
      ensure_legendary!
      self.class::LEGENDARY_PLAN_TOTAL_SUPPLY
    end

    def sold_percent
      ensure_legendary!
      return 0 if plans_total_supply.zero?

      @sold_percent ||= active_subscriptions_count * 100 / plans_total_supply
    end

    def for_sale_count
      ensure_legendary!
      @for_sale_count ||= plans_total_supply - active_subscriptions_count
    end

    def available?
      ensure_legendary!
      (active_subscriptions_count >= 0) && (active_subscriptions_count < total_supply)
    end

    private

    def legendary?
      @legendary ||= name == self.class::LEGENDARY_PLAN
    end

    def ensure_legendary!
      raise 'This method is only available for the Legendary Badger NFT plan' unless legendary?
    end
  end
end
