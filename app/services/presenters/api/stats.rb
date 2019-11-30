module Presenters
  module Api
    class Stats < BaseService
      def initialize(
        get_exchange_api: ExchangeApi::Get.new,
        api_keys_repository: ApiKeysRepository.new
      )

        @get_exchange_api = get_exchange_api
        @api_keys_repository = api_keys_repository
      end

      def call(bot:, transactions:)
        return {} if transactions.empty?

        api_key = @api_keys_repository.for_bot(bot.user_id, bot.exchange_id)
        api = @get_exchange_api.call(api_key)
        current_price = api.current_price(bot.settings)

        transactions_price_sum = transactions.sum(&:price)
        transactions_amount_sum = transactions.sum(&:amount)
        average_price = transactions_price_sum / transactions.length
        total_invested = transactions.sum('rate * amount')
        current_value =  current_price * transactions_amount_sum
        profit_loss = current_value - total_invested

        {
          bought: "#{transactions_amount_sum} BTC",
          spent: "$#{transactions_price_sum}".slice(0, 8),
          averagePrice: "$#{average_price}".slice(0, 8),
          currentValue: current_value,
          profitLoss: {
            positive: profit_loss.positive?,
            value: "#{profit_loss.positive? ? '+' : '-'}#{profit_loss}".slice(0, 8)
          }
        }
      end
    end
  end
end
