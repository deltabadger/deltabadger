module Presenters
  module Api
    class Stats < BaseService
      def initialize(
          get_exchange_api: ExchangeApi::Clients::GetValidator.new,
          api_keys_repository: ApiKeysRepository.new
      )

        @get_exchange_api = get_exchange_api
        @api_keys_repository = api_keys_repository
      end

      def call(bot:, transactions:)
        return {} if transactions.empty?

        api_key = @api_keys_repository.for_bot(bot.user_id, bot.exchange_id)
        api = @get_exchange_api.call(api_key, bot.order_type)

        current_price_result = api.current_price(bot.currency)
        current_price = current_price_result.or(transactions.last.rate)
        transactions_amount_sum = transactions.sum(&:amount)
        total_invested = transactions.sum(&:price).ceil(5)

        average_price =  total_invested / transactions_amount_sum
        current_value = current_price * transactions_amount_sum

        {
          bought: bought_format(transactions_amount_sum),
          totalInvested: price_format(total_invested, bot),
          averagePrice: price_format(average_price, bot),
          currentValue: price_format(current_value, bot),
          profitLoss: profit_loss_format(current_value, total_invested, bot),
          currentPriceAvailable: current_price_result.success?
        }
      end

      private

      def bought_format(transactions_amount_sum)
        "#{transactions_amount_sum.floor(6)} BTC"
      end

      def price_format(price, bot)
        "#{price.floor(2)} #{bot.currency}"
      end

      def profit_loss_format(current_value, total_invested, bot)
        profit_loss = current_value - total_invested
        profit_loss_percentage = (1 - current_value / total_invested) * 100

        positive = !profit_loss.negative? # 0 is not negative also

        {
          positive: positive,
          value: "#{positive ? '+' : '-'}#{profit_loss.floor(2).abs} "\
            "#{bot.currency} "\
            "(#{positive ? '+' : '-'}#{profit_loss_percentage.floor(2).abs}%)"
        }
      end
    end
  end
end
