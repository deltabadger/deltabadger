module Presenters
  module Api
    class Stats < BaseService
      def initialize(get_markets: ExchangeApi::Markets::Get.new)
        @get_exchange_market = get_markets
      end

      def call(bot:, transactions:)
        return {} if transactions.empty?

        market = @get_exchange_market.call(bot.exchange_id)

        market_symbol = market.symbol(bot.base, bot.quote)
        current_price_result = market.current_price(market_symbol)
        current_price = current_price_result.or(transactions.last.rate)
        transactions_amount_sum = transactions.sum(&:amount)
        total_invested = transactions.sum(&:price).ceil(8)

        average_price =  total_invested / transactions_amount_sum
        current_value = current_price * transactions_amount_sum

        {
          bought: bought_format(transactions_amount_sum),
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
        profit_loss_percentage = (1 - current_value / total_invested) * 100

        positive = !profit_loss.negative? # 0 is not negative also

        {
          positive: positive,
          value: "#{positive ? '+' : '-'}#{profit_loss.floor(8).abs}",
          percentage: "(#{positive ? '+' : '-'}#{profit_loss_percentage.floor(2).abs}%)"
        }
      end
    end
  end
end
