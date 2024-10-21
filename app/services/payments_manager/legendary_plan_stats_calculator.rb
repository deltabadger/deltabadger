module PaymentsManager
  class LegendaryPlanStatsCalculator < BaseService
    LEGENDARY_PLANS_TOTAL_SUPPLY = 1000

    def call
      data = {
        legendary_plans_total_supply: legendary_plans_total_supply,
        legendary_plans_sold_count: legendary_plans_sold_count,
        legendary_plans_sold_percent: legendary_plans_sold_percent,
        legendary_plans_for_sale_count: legendary_plans_for_sale_count,
        legendary_plan_discount: legendary_plan_discount
      }
      Result::Success.new(data)
    end

    private

    def legendary_plan_discount
      [0, legendary_plans_for_sale_count * 10].max
    end

    def legendary_plans_sold_count
      @legendary_plans_sold_count ||= Subscription.number_of_active_subscriptions('legendary')
    end

    def legendary_plans_total_supply
      @legendary_plans_total_supply ||= LEGENDARY_PLANS_TOTAL_SUPPLY
    end

    def legendary_plans_sold_percent
      return 0 if legendary_plans_total_supply.zero?

      @legendary_plans_sold_percent ||= legendary_plans_sold_count * 100 / legendary_plans_total_supply
    end

    def legendary_plans_for_sale_count
      @legendary_plans_for_sale_count ||= legendary_plans_total_supply - legendary_plans_sold_count
    end
  end
end
