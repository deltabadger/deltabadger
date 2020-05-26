module Presenters
  module Api
    class Stats < BaseService
      PRICE_ERROR = 'Current BTC price not available'.freeze

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

        current_price_result = api.current_price(bot.settings)
        transactions_amount_sum = transactions.sum(&:amount)
        total_invested = transactions.sum(&:price).ceil(5)

        average_price =  total_invested / transactions_amount_sum
        current_value_result = calc_current_value(current_price_result, transactions_amount_sum)

        {
          bought: bought_format(transactions_amount_sum),
          totalInvested: price_format(total_invested, bot),
          averagePrice: price_format(average_price, bot),
          currentValue: current_value_format(current_value_result, bot),
          profitLoss: profit_loss_format(current_value_result, total_invested, bot)
        }
      end

      private

      def calc_current_value(current_price_result, transactions_amount_sum)
        return current_price_result if current_price_result.failure?

        Result::Success.new(current_price_result.data * transactions_amount_sum)
      end

      def bought_format(transactions_amount_sum)
        "#{transactions_amount_sum.floor(6)} BTC"
      end

      def price_format(price, bot)
        "#{price.floor(2)} #{bot.currency}"
      end

      def current_value_format(current_value_result, bot)
        if current_value_result.success?
          "#{current_value_result.data.floor(2)} #{bot.currency}"
        else
          PRICE_ERROR
        end
      end

      def profit_loss_format(current_value_result, total_invested, bot)
        return { positive: false, value: PRICE_ERROR } if current_value_result.failure?

        current_value = current_value_result.data
        profit_loss = current_value - total_invested
        profit_loss_percentage = (1 - current_value / total_invested) * 100

        {
          positive: profit_loss.positive?,
          value: "#{profit_loss.positive? ? '+' : '-'}#{profit_loss.floor(2).abs} "\
            "#{bot.currency} "\
            "(#{profit_loss.positive? ? '+' : '-'}#{profit_loss_percentage.floor(2).abs}%)"
        }
      end
    end
  end
end
