module Presenters
  module Api
    class Stats < BaseService
      def initialize(get_markets: ExchangeApi::Markets::Get.new)
        @get_exchange_market = get_markets
      end

      def call(bot:, daily_transaction_aggregates:)
        return {} if daily_transaction_aggregates.empty?

        market = @get_exchange_market.call(bot.exchange_id)

        market_symbol = market.symbol(bot.base, bot.quote)
        current_price_result = market.current_price(market_symbol)
        last_aggregate = daily_transaction_aggregates.last
        current_price = current_price_result.or(last_aggregate.rate)

        total_amount = last_aggregate.total_amount
        total_invested = last_aggregate.total_invested

        average_price = total_invested / total_amount
        current_value = last_aggregate.total_value

        {
          bought: bought_format(total_amount),
          totalInvested: price_format(total_invested),
          averagePrice: price_format(average_price),
          currentValue: price_format(current_value),
          profitLoss: profit_loss_format(current_value, total_invested),
          currentPriceAvailable: current_price_result.success?
        }
      end

      private

      def bought_format(transactions_amount_sum)
        transactions_amount_sum.floor(8).to_s
      end

      def price_format(price)
        price.floor(8).to_s
      end

      def profit_loss_format(current_value, total_invested)
        profit_loss = current_value - total_invested
        profit_loss_percentage = ((1 - current_value / total_invested) * 100).floor(2)

        positive = profit_loss >= 0
        sign = positive ? '+' : '-'

        {
          positive: positive,
          value: "#{sign}#{profit_loss.abs.floor(8)}",
          percentage: "(#{sign}#{profit_loss_percentage.abs}%)"
        }
      end
    end
  end
end
