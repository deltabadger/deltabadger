module PaymentsManager
  class LegendaryBadgerStatsCalculator < BaseService
    LEGENDARY_BADGER_TOTAL_SUPPLY = 1000

    def call
      data = {
        legendary_badger_total_supply: legendary_badger_total_supply,
        sold_legendary_badger_count: sold_legendary_badger_count,
        sold_legendary_badger_percent: sold_legendary_badger_percent,
        for_sale_legendary_badger_count: for_sale_legendary_badger_count,
        legendary_badger_discount: legendary_badger_discount
      }
      Result::Success.new(data)
    end

    private

    def legendary_badger_discount
      [0, for_sale_legendary_badger_count].max
    end

    def sold_legendary_badger_count
      @sold_legendary_badger_count ||= SubscriptionsRepository.new.number_of_active_subscriptions('legendary_badger')
    end

    def legendary_badger_total_supply
      @legendary_badger_total_supply ||= LEGENDARY_BADGER_TOTAL_SUPPLY
    end

    def sold_legendary_badger_percent
      return 0 if legendary_badger_total_supply.zero?

      @sold_legendary_badger_percent ||= sold_legendary_badger_count * 100 / legendary_badger_total_supply
    end

    def for_sale_legendary_badger_count
      @for_sale_legendary_badger_count ||= legendary_badger_total_supply - sold_legendary_badger_count
    end
  end
end
