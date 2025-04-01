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
        
        # Fetch sums directly from the database
        sums = bot.daily_transaction_aggregates
                  .select("SUM(amount) as total_amount, SUM(amount * rate) as total_invested_value")
                  .first

        # Handle case where there might be no aggregates yet
        transactions_amount_sum = sums&.total_amount || 0.0
        total_invested = sums&.total_invested_value || 0.0

        # Avoid division by zero if amount sum is zero
        average_price = transactions_amount_sum.positive? ? (total_invested / transactions_amount_sum) : 0.0

        # Use the last transaction rate as fallback if current price fetch fails
        current_price = current_price_result.or(bot.daily_transaction_aggregates.order(created_at: :desc).first&.rate || 0.0)

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
