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
        current_price = api.current_price(bot.settings)*10

        transactions_amount_sum = transactions.sum(&:amount)
        transactions_price_sum  = transactions.sum(&:price).ceil(5)

        average_price = transactions_price_sum / transactions.length
        total_invested = transactions_price_sum
        current_value  = transactions_amount_sum * current_price
        profit_loss = current_value - total_invested
        profit_loss_percentage = (1 - current_value/total_invested)*100

        {
          bought: "#{transactions_amount_sum.floor(2)} BTC",
          totalInvested: "#{total_invested.floor(2)} #{bot.currency}",
          averagePrice: "#{average_price.floor(2)} #{bot.currency}",
          currentValue: "#{current_value.floor(2)} #{bot.currency}",
          profitLoss: {
            positive: profit_loss.positive?,
            value: "#{profit_loss.positive? ? '+' : '-'}#{profit_loss.floor(2).abs} #{bot.currency} (#{profit_loss.positive? ? '+' : '-'}#{profit_loss_percentage.floor(2).abs}%)"
          }
        }
      end
    end
  end
end
