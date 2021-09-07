module ExchangeApi
  module Traders
    module Bitstamp
      class MarketTrader < ExchangeApi::Traders::Bitstamp::BaseTrader
        def buy(base:, quote:, price:, force_smart_intervals:, smart_intervals_value:)
          symbol = @market.symbol(base, quote)
          buy_params = get_params(symbol, price, force_smart_intervals, smart_intervals_value, true)
          return buy_params unless buy_params.success?

          place_order(buy_params.data, 'buy', symbol, 'market')
        end

        def sell(base:, quote:, price:, force_smart_intervals:, smart_intervals_value:, is_legacy:)
          symbol = @market.symbol(base, quote)
          sell_params = get_params(symbol, price, force_smart_intervals, smart_intervals_value, is_legacy)
          return sell_params unless sell_params.success?

          place_order(sell_params.data, 'sell', symbol, 'market')
        end

        private

        def get_params(symbol, price, force_smart_intervals, smart_intervals_value, price_in_quote)
          rate = @market.current_bid_price(symbol)
          return rate unless rate.success?

          volume = smart_volume(symbol, price, rate.data, force_smart_intervals, smart_intervals_value, price_in_quote)
          return volume unless volume.success?

          Result::Success.new(amount: volume.data)
        end
      end
    end
  end
end
