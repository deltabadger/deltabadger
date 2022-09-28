require 'result'

module ExchangeApi
  module Traders
    module Zonda
      class LimitTrader < ExchangeApi::Traders::Zonda::BaseTrader
        def buy(base:, quote:, price:, percentage:, force_smart_intervals:, smart_intervals_value:)
          symbol = @market.symbol(base, quote)
          buy_params = get_buy_params(symbol, price, percentage, force_smart_intervals, smart_intervals_value)
          return buy_params unless buy_params.success?

          place_order(symbol, buy_params.data)
        end

        def sell(base:, quote:, price:, percentage:, force_smart_intervals:, smart_intervals_value:, is_legacy:)
          symbol = @market.symbol(base, quote)
          sell_params = get_sell_params(symbol, price, percentage, force_smart_intervals, smart_intervals_value, is_legacy)
          return sell_params unless sell_params.success?

          place_order(symbol, sell_params.data)
        end

        private

        def place_order(symbol, params)
          response = super
          return response unless response.success?

          Result::Success.new(
            response.data.merge(rate: params[:rate], amount: params[:amount])
          )
        rescue StandardError
          Result::Failure.new('Could not make Zonda order', **RECOVERABLE)
        end

        def parse_response(response)
          if response.fetch('status') == 'Ok'
            Result::Success.new(
              offer_id: response.fetch('offerId')
            )
          else
            error_to_failure(response.fetch('errors'))
          end
        end

        def get_buy_params(symbol, price, percentage, force_smart_intervals, smart_intervals_value)
          rate = @market.current_ask_price(symbol)
          return rate unless rate.success?

          limit_rate = rate_percentage(symbol, rate.data, -percentage)
          return limit_rate unless limit_rate.success?

          price_above_minimums = transaction_price(symbol, price, force_smart_intervals, smart_intervals_value, true)
          return price_above_minimums unless price_above_minimums.success?

          amount = transaction_volume(symbol, price_above_minimums.data, limit_rate.data, true)
          return amount unless amount.success?

          Result::Success.new(common_order_params.merge(
                                offerType: 'buy',
                                amount: amount.data,
                                rate: limit_rate.data
                              ))
        end

        def get_sell_params(symbol, price, percentage, force_smart_intervals, smart_intervals_value, price_in_quote)
          rate = @market.current_bid_price(symbol)
          return rate unless rate.success?

          limit_rate = rate_percentage(symbol, rate.data, percentage)
          return limit_rate unless limit_rate.success?

          price_above_minimums = transaction_price(symbol, price, force_smart_intervals, smart_intervals_value, price_in_quote)
          return price_above_minimums unless price_above_minimums.success?

          amount = transaction_volume(symbol, price_above_minimums.data, limit_rate.data, price_in_quote)
          return amount unless amount.success?

          Result::Success.new(common_order_params.merge(
                                offerType: 'sell',
                                amount: price_above_minimums.data,
                                rate: limit_rate.data
                              ))
        end

        def rate_percentage(symbol, rate, percentage)
          rate_decimals = @market.quote_decimals(symbol)
          return rate_decimals unless rate_decimals.success?

          Result::Success.new((rate * (1 + percentage / 100)).ceil(rate_decimals.data))
        end

        def common_order_params
          super.merge(mode: 'limit')
        end
      end
    end
  end
end
