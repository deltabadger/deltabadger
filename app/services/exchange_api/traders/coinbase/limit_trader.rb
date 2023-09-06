require 'securerandom'

module ExchangeApi
  module Traders
    module Coinbase
      class LimitTrader < ExchangeApi::Traders::Coinbase::BaseTrader
        def buy(base:, quote:, price:, percentage:, force_smart_intervals:, smart_intervals_value:)
          symbol = @market.symbol(base, quote)
          buy_params = get_buy_params(symbol, price, percentage, force_smart_intervals, smart_intervals_value)
          return buy_params unless buy_params.success?

          place_order(buy_params.data)
        end

        def sell(base:, quote:, price:, percentage:, force_smart_intervals:, smart_intervals_value:, is_legacy:)
          symbol = @market.symbol(base, quote)
          sell_params = get_sell_params(symbol, price, percentage, force_smart_intervals, smart_intervals_value, is_legacy)
          return sell_params unless sell_params.success?

          place_order(sell_params.data)
        end

        private

        def get_buy_params(symbol, price, percentage, force_smart_intervals, smart_intervals_value)
          rate = @market.current_ask_price(symbol)
          return rate unless rate.success?

          limit_rate = rate_percentage(symbol, rate.data, -percentage)
          return limit_rate unless limit_rate.success?

          volume = smart_volume(symbol, price, limit_rate.data, force_smart_intervals, smart_intervals_value, true)
          return volume unless volume.success?

          Result::Success
            .new(
              product_id: symbol,
              client_order_id: SecureRandom.uuid,
              order_configuration: {
                limit_limit_gtc: {
                  base_size: volume.data.to_f.to_s,
                  limit_price: limit_rate.data.to_f.to_s
                }
              },
              side: 'BUY')
        end

        def get_sell_params(symbol, price, percentage, force_smart_intervals, smart_intervals_value, price_in_quote)
          rate = @market.current_bid_price(symbol)
          return rate unless rate.success?

          limit_rate = rate_percentage(symbol, rate.data, percentage)
          return limit_rate unless limit_rate.success?

          volume = smart_volume(symbol, price, limit_rate.data, force_smart_intervals, smart_intervals_value, price_in_quote)
          return volume unless volume.success?

          Result::Success
            .new(
              product_id: symbol,
              client_order_id: SecureRandom.uuid,
              order_configuration: {
                limit_limit_gtc: {
                  base_size: volume.data.to_f.to_s,
                  limit_price: limit_rate.data.to_f.to_s
                }
              },
              side: 'SELL')
        end

        def rate_percentage(symbol, rate, percentage)
          rate_decimals = @market.quote_decimals(symbol)
          return rate_decimals unless rate_decimals.success?

          Result::Success.new((rate * (1 + percentage / 100)).ceil(rate_decimals.data))
        end
      end
    end
  end
end